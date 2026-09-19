//! Client-router route tables (LA.6b): ONE walker for Angular Router, React
//! Router and Vue Router, run once per TS-family file.
//!
//! Before this module each framework extractor ran its own `path:` substring
//! scan and the engine ran all three on every plain `.ts` file, so one literal
//! was minted up to three times, any object with a `path:` key became a page
//! (`{ path: 'tmp/out.txt', mode: 1 }`), child tables were never composed
//! (`{ path: 'admin', children: [{ path: 'users' }] }` gave `/users`), and no
//! route said which component renders it.
//!
//! # Object tables (`scan_route_records`)
//!
//! One forward lexer pass over the source tracks `{` / `[` / `(` nesting while
//! skipping strings, template literals (their `${...}` bodies are lexed as
//! code), comments and regex literals, and records every object literal's
//! TOP-LEVEL keys with their value spans. The pass is linear, so no per-record
//! cap is needed to bound it (a cap would drop a large parent table and break
//! composition); the per-value readers are clipped instead ([`MAX_VALUE`]).
//!
//! An object with a literal `path` is a route RECORD only when it also has a
//! router key ([`ROUTER_KEYS`]); otherwise it is counted `rejected` (Hapi
//! `{ method, path, handler }`, mongoose `{ path, select }`, cookie options,
//! form configs). A React `{ index: true, element }` record takes its parent's
//! path. A record's parent is the smallest enclosing record whose `children`
//! value span contains it; a relative path composes onto the parent's, an
//! absolute one (leading `/`) stands alone. A pathless layout object is not a
//! record, so its children compose straight through it.
//!
//! # JSX tables (`scan_jsx_routes`)
//!
//! React Router `<Routes>` trees: `<Route>` tags in source order with a stack
//! of the open (non self-closing) ones; `path` composes onto the stack top like
//! an object child, `index` takes the parent path, `</Route>` pops.
//!
//! # Emission (`emit_nav_routes`)
//!
//! One ROUTE per unique composed path, first-seen order, qname
//! [`nav_route_qname`], cells ROUTE_METHOD `GET` + the A3.4 nav ORIGIN. A
//! handler becomes a `HANDLED_BY` ref (bound by the graph builder's
//! `resolve_refs`: import binding, same-module symbol, then the unique-global
//! fallback, which returns None on a tie). A redirect becomes a
//! `NAVIGATES_TO` ref from the ROUTE, its target made absolute: LA.6a's link
//! contract (graph/src/nav.rs).

use std::collections::HashSet;

use glia_code_domain::{
    CallQualifier, CodeNav, GRAPH_TYPE, UnresolvedRef, cell_type, edge_category, line_of,
    node_kind,
};
use glia_core::{Cell, CellPayload, Confidence, EdgeCategoryId, Node, NodeId, RepoId};

use crate::react::nav_route_origin_cell;

/// Keys that make an object with a `path` a router record.
const ROUTER_KEYS: &[&str] = &[
    "component",
    "loadComponent",
    "loadChildren",
    "children",
    "redirectTo",
    "redirect",
    "element",
    "Component",
    "lazy",
    "index",
];

/// Byte cap on any single value read (a handler / element / import
/// expression) and on one JSX opening tag. Everything past it is ignored.
const MAX_VALUE: usize = 1024;
const MAX_TAG: usize = 4096;

/// One client-router route declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteRecord {
    /// Composed path with a leading `/` (`/admin/users`).
    pub path: String,
    /// The component that renders the route (`HomeComponent`, `Pages.Home`).
    pub handler: Option<String>,
    /// Absolute redirect target (`/login`).
    pub redirect: Option<String>,
    /// The last segment is a wildcard (`**`, `*`, `:x*`, `[...x]`).
    pub catchall: bool,
    /// 0-based row of the declaration (the route object's `{`, the `<Route`
    /// tag, a Next page's default export): the site line of its HANDLED_BY /
    /// NAVIGATES_TO refs (LC.3b).
    pub line: u32,
}

/// What a file's route tables contribute to its `FileParse`, plus the
/// counters behind the engine's `[extract] nav-routes marked:` line.
#[derive(Debug, Default)]
pub struct NavRouteOut {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    pub refs: Vec<UnresolvedRef>,
    /// Nav ROUTE nodes minted (one per unique composed path).
    pub nav_routes: usize,
    /// HANDLED_BY refs emitted.
    pub bound: usize,
    /// NAVIGATES_TO (redirect) refs emitted.
    pub redirects: usize,
    /// Records whose path was composed onto a parent record's.
    pub children: usize,
    /// `path` objects refused: no router key, or not a URL path.
    pub rejected: usize,
    /// Nav ROUTE nodes whose path ends in a wildcard.
    pub catchalls: usize,
}

/// The ONE builder for a client-router page qname (LB.4c): `page:<path>`,
/// its own namespace so a page never shares a NodeId with a same-repo server
/// route `GET <path>`. The display name is the bare path.
pub fn nav_route_qname(path: &str) -> String {
    format!("page:{path}")
}

/// Every route table in one TS-family file (`.ts` / `.tsx` / `.js` / `.jsx` /
/// `.vue`, whole source): object tables always, JSX `<Route>` trees when the
/// source holds `<Route`.
pub fn extract_route_tables(source: &str, module_id: NodeId, repo: RepoId) -> NavRouteOut {
    let mut scan = scan_route_records(source);
    if source.contains("<Route") {
        let jsx = scan_jsx_routes(source);
        scan.records.extend(jsx.records);
        scan.children += jsx.children;
        scan.rejected += jsx.rejected;
    }
    let mut out = emit_nav_routes(&scan.records, module_id, repo);
    out.children = scan.children;
    out.rejected = scan.rejected;
    out
}

/// Mint the ROUTE nodes and the HANDLED_BY / NAVIGATES_TO refs for `recs`
/// (see the module doc). `pub` so Next.js pages (LA.6d) graft through it.
pub fn emit_nav_routes(recs: &[RouteRecord], module_id: NodeId, repo: RepoId) -> NavRouteOut {
    let mut out = NavRouteOut::default();
    let mut seen_paths: HashSet<&str> = HashSet::new();
    let mut seen_refs: HashSet<(NodeId, &str, EdgeCategoryId)> = HashSet::new();
    for rec in recs {
        let qname = nav_route_qname(&rec.path);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &qname);
        if seen_paths.insert(rec.path.as_str()) {
            out.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![
                    Cell {
                        kind: cell_type::ROUTE_METHOD,
                        payload: CellPayload::Text("GET".to_string()),
                    },
                    nav_route_origin_cell(),
                ],
            });
            out.nav
                .record(id, &rec.path, &qname, node_kind::ROUTE, None);
            out.nav_routes += 1;
            if rec.catchall {
                out.catchalls += 1;
            }
        }
        if let Some(h) = rec.handler.as_deref()
            && seen_refs.insert((id, h, edge_category::HANDLED_BY))
        {
            out.refs.push(UnresolvedRef {
                from: id,
                from_module: module_id,
                qualifier: handler_qualifier(h),
                category: edge_category::HANDLED_BY,
                line: rec.line,
            });
            out.bound += 1;
        }
        if let Some(t) = rec.redirect.as_deref()
            && seen_refs.insert((id, t, edge_category::NAVIGATES_TO))
        {
            out.refs.push(UnresolvedRef {
                from: id,
                from_module: module_id,
                qualifier: CallQualifier::Bare(t.to_string()),
                category: edge_category::NAVIGATES_TO,
                line: rec.line,
            });
            out.redirects += 1;
        }
    }
    out
}

/// `Pages.Home` binds through the `Pages` namespace import; a plain name is
/// `Bare`.
fn handler_qualifier(h: &str) -> CallQualifier {
    match h.split_once('.') {
        Some((base, name)) if !base.is_empty() && !name.is_empty() && !name.contains('.') => {
            CallQualifier::Attribute {
                base: base.to_string(),
                name: name.to_string(),
            }
        }
        _ => CallQualifier::Bare(h.to_string()),
    }
}

/// Records found by one walker, in source order.
#[derive(Debug, Default)]
struct Scan {
    records: Vec<RouteRecord>,
    children: usize,
    rejected: usize,
}

// ----------------------------------------------------------------------------
// Paths
// ----------------------------------------------------------------------------

/// Reject path strings that look like JS code rather than URLs. The bare
/// `path:` substring scan this module replaced caught `path: '/foo'` in
/// arbitrary JS files (Hapi's `'Invalid path: ...'` error string was the
/// canonical FP that emitted ~80 garbage routes per hapi/lib file). Url paths
/// don't contain newlines, parens, semicolons, or quotes. (Moved unchanged
/// from react.rs.)
fn looks_like_url_path(p: &str) -> bool {
    if p.is_empty() || p.len() > 256 || !p.starts_with('/') {
        return false;
    }
    p.chars().all(|c| match c {
        '\n' | '\r' | '\t' | ' ' => false,
        c if c.is_ascii_control() => false,
        '(' | ')' | ';' | '"' | '\'' | '`' | ',' => false,
        _ => true,
    })
}

/// A composed route path worth a node: [`looks_like_url_path`] with the
/// parenthesised regex of a `:param(...)` segment (vue-router
/// `/:pathMatch(.*)*`, `/:id(\\d+)`) masked out first, and no `${...}`
/// template-source expression (glia-v2 G8).
fn is_route_path(p: &str) -> bool {
    if p.contains("${") {
        return false;
    }
    let masked: Vec<String> = p
        .split('/')
        .map(
            |seg| match (seg.starts_with(':'), seg.find('('), seg.rfind(')')) {
                (true, Some(open), Some(close)) if open < close => {
                    let head = seg.get(..open).unwrap_or("");
                    let tail = seg.get(close + 1..).unwrap_or("");
                    format!("{head}{tail}")
                }
                _ => seg.to_string(),
            },
        )
        .collect();
    looks_like_url_path(&masked.join("/"))
}

/// Collapse `//`, drop `.` segments, resolve `..`, force the leading `/`, drop
/// a trailing one (the root stays `/`).
fn normalize(p: &str) -> String {
    let mut segs: Vec<&str> = Vec::new();
    for s in p.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    if segs.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segs.join("/"))
    }
}

/// `child` under `parent`: an absolute child stands alone, a relative one
/// (including `''`) composes onto the parent path.
fn compose(parent: &str, child: &str) -> String {
    if child.starts_with('/') {
        normalize(child)
    } else {
        normalize(&format!("{parent}/{child}"))
    }
}

/// A navigation target made absolute against `base` (query and fragment
/// stripped: LA.6a's link contract). `None` for an external URL.
fn absolute_link(base: &str, target: &str) -> Option<String> {
    let t = target.split(['?', '#']).next().unwrap_or(target).trim();
    if t.contains("://") || t.starts_with("//") || t.starts_with("mailto:") {
        return None;
    }
    Some(compose(base, t))
}

/// The last segment is a wildcard: Angular `**`, React `*`, vue-router
/// `:pathMatch(.*)*` / `:x*`, Next `[...x]` / `[[...x]]`.
fn is_catchall(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or("");
    last == "*"
        || last == "**"
        || (last.len() > 2 && last.starts_with(':') && last.ends_with('*'))
        || last.starts_with("[...")
        || last.starts_with("[[...")
}

// ----------------------------------------------------------------------------
// Value readers
// ----------------------------------------------------------------------------

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80
}

/// The first `n` bytes of `s`, never splitting a char.
fn clip(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut i = n;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    s.get(..i).unwrap_or("")
}

/// Index just past the closing quote of the literal opening at `i`, or the
/// newline / end that terminates an unclosed `'` / `"` literal. Backtick
/// literals may span lines.
fn skip_quoted(b: &[u8], i: usize) -> usize {
    let q = b[i];
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            c if c == q => return j + 1,
            b'\n' if q != b'`' => return j,
            _ => j += 1,
        }
    }
    b.len()
}

/// `v` is exactly one string literal (`'x'`, `"x"`, a backtick literal):
/// its raw contents.
fn literal(v: &str) -> Option<String> {
    let v = v.trim();
    let b = v.as_bytes();
    let q = *b.first()?;
    if !matches!(q, b'\'' | b'"' | b'`') {
        return None;
    }
    let end = skip_quoted(b, 0);
    if end != b.len() || end < 2 || b[end - 1] != q {
        return None;
    }
    v.get(1..end - 1).map(str::to_string)
}

/// `v` is one identifier or dotted identifier path (`X`, `Pages.Home`).
fn ident_path(v: &str) -> Option<String> {
    let v = v.trim();
    let ok = !v.is_empty()
        && v.bytes().all(|c| is_ident_byte(c) || c == b'.')
        && v.split('.').all(|s| !s.is_empty())
        && !v.as_bytes()[0].is_ascii_digit();
    ok.then(|| v.to_string())
}

/// The component a lazy import names: `.then(m => m.X)` / `.then(({ X }) =>
/// X)` gives `X`; otherwise the imported file's stem ([`stem_name`]).
fn lazy_import_handler(v: &str) -> Option<String> {
    let v = clip(v, MAX_VALUE);
    let at = v.find("import(")?;
    let arg = v.get(at + "import(".len()..)?.trim_start();
    let quote = *arg.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"' | b'`') {
        return None;
    }
    let end = skip_quoted(arg.as_bytes(), 0);
    let spec = literal(arg.get(..end)?)?;
    if let Some(then) = v.find(".then(")
        && let Some(arrow) = v.get(then..).and_then(|t| t.find("=>"))
        && let Some(body) = v.get(then + arrow + 2..)
    {
        let body = body.trim_start();
        let tok_len = body
            .bytes()
            .take_while(|&c| is_ident_byte(c) || c == b'.')
            .count();
        let tok = body.get(..tok_len).unwrap_or("");
        // `m => m.X` names the export; a bare `X` counts only when it is a
        // component name (`({ X }) => X`), never the parameter itself.
        let name = match tok.rsplit_once('.') {
            Some((_, n)) => n,
            None if tok.as_bytes().first().is_some_and(u8::is_ascii_uppercase) => tok,
            None => "",
        };
        if !name.is_empty() && name != "default" {
            return Some(name.to_string());
        }
    }
    stem_name(&spec)
}

/// `./views/Home.vue` -> `Home`, `./pages/Reports` -> `Reports`,
/// `./pages/users/index` -> `users`, and the Angular CLI convention
/// `./user-list.component` -> `UserListComponent`.
fn stem_name(spec: &str) -> Option<String> {
    let mut segs = spec
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..");
    let mut last = segs.next_back()?;
    const EXTS: &[&str] = &[".vue", ".tsx", ".ts", ".jsx", ".js", ".mjs", ".cjs"];
    for ext in EXTS {
        if let Some(s) = last.strip_suffix(ext) {
            last = s;
            break;
        }
    }
    if last == "index" {
        last = segs.next_back()?;
    }
    if last.is_empty() {
        return None;
    }
    if !last.contains('.') {
        return Some(last.to_string());
    }
    let pascal: String = last
        .split(['.', '-', '_'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut cs = p.chars();
            match cs.next() {
                Some(c) => c.to_uppercase().chain(cs).collect::<String>(),
                None => String::new(),
            }
        })
        .collect();
    (!pascal.is_empty()).then_some(pascal)
}

/// What a route element renders.
#[derive(Debug, PartialEq, Eq)]
enum Element {
    Handler(String),
    /// A `<Navigate to="...">`'s raw target.
    Redirect(String),
}

/// React built-ins that wrap a route's page but are never the page.
fn is_react_wrapper(name: &str) -> bool {
    matches!(
        name.strip_prefix("React.").unwrap_or(name),
        "Suspense" | "Fragment" | "StrictMode"
    )
}

/// The page a JSX element expression renders: the first capitalised tag,
/// looking through `<Suspense>` / `<>` / `<Fragment>` wrappers; a
/// `<Navigate to>` is a redirect instead.
fn jsx_element(v: &str) -> Option<Element> {
    let v = clip(v, MAX_VALUE);
    let mut from = 0;
    while let Some(rel) = v.get(from..).and_then(|s| s.find('<')) {
        let lt = from + rel;
        let Some(tag) = parse_tag(v, lt) else {
            from = lt + 1;
            continue;
        };
        if tag.name.is_empty() || is_react_wrapper(&tag.name) {
            from = tag.end;
            continue;
        }
        if !tag.name.as_bytes()[0].is_ascii_uppercase() {
            return None;
        }
        if tag.name == "Navigate" || tag.name.ends_with(".Navigate") {
            return tag.attr("to").and_then(attr_literal).map(Element::Redirect);
        }
        return Some(Element::Handler(tag.name));
    }
    None
}

// ----------------------------------------------------------------------------
// JSX tags
// ----------------------------------------------------------------------------

/// One JSX opening tag.
#[derive(Debug)]
struct Tag {
    name: String,
    /// `(name, raw value)`: `"x"` / `'x'` / `{...}` with its delimiters, or
    /// `None` for a bare boolean attribute.
    attrs: Vec<(String, Option<String>)>,
    /// Index just past the tag's `>`.
    end: usize,
    self_closing: bool,
}

impl Tag {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.as_deref())
    }

    fn has(&self, name: &str) -> bool {
        self.attrs.iter().any(|(n, _)| n == name)
    }
}

/// Index just past the `}` matching the `{` at `i`, skipping string and
/// template literals. `None` when unbalanced within `limit`.
fn skip_braces(b: &[u8], i: usize, limit: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = i;
    while j < b.len().min(limit) {
        match b[j] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(j + 1);
                }
            }
            b'\'' | b'"' | b'`' => {
                j = skip_quoted(b, j);
                continue;
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// Parse the JSX opening tag whose `<` is at `lt`. `None` when it is not a
/// well-formed tag within [`MAX_TAG`] bytes.
fn parse_tag(s: &str, lt: usize) -> Option<Tag> {
    let b = s.as_bytes();
    let limit = lt.saturating_add(MAX_TAG);
    let mut j = lt + 1;
    let name_len = b
        .get(j..)?
        .iter()
        .take_while(|&&c| is_ident_byte(c) || c == b'.' || c == b'-' || c == b':')
        .count();
    let name = s.get(j..j + name_len)?.to_string();
    j += name_len;
    let mut attrs = Vec::new();
    loop {
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= b.len().min(limit) {
            return None;
        }
        match b[j] {
            b'>' => {
                return Some(Tag {
                    name,
                    attrs,
                    end: j + 1,
                    self_closing: false,
                });
            }
            b'/' if b.get(j + 1) == Some(&b'>') => {
                return Some(Tag {
                    name,
                    attrs,
                    end: j + 2,
                    self_closing: true,
                });
            }
            b'{' => {
                // `{...spread}` attribute.
                j = skip_braces(b, j, limit)?;
                continue;
            }
            _ => {}
        }
        let at = j;
        while j < b.len() && (is_ident_byte(b[j]) || b[j] == b'-' || b[j] == b':') {
            j += 1;
        }
        if j == at {
            return None;
        }
        let attr = s.get(at..j)?.to_string();
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if b.get(j) != Some(&b'=') {
            attrs.push((attr, None));
            continue;
        }
        j += 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let vstart = j;
        match b.get(j) {
            Some(b'\'' | b'"') => j = skip_quoted(b, j),
            Some(b'{') => j = skip_braces(b, j, limit)?,
            Some(_) => {
                while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                    j += 1;
                }
            }
            None => return None,
        }
        attrs.push((attr, s.get(vstart..j).map(str::to_string)));
    }
}

/// The inner expression of a `{...}` attribute value, or the value itself.
fn attr_expr(v: &str) -> &str {
    let t = v.trim();
    t.strip_prefix('{')
        .and_then(|x| x.strip_suffix('}'))
        .unwrap_or(t)
        .trim()
}

/// A literal attribute value: `"x"`, `'x'`, `{"x"}`, `{'x'}`, a backtick
/// literal in braces.
fn attr_literal(v: &str) -> Option<String> {
    literal(attr_expr(v))
}

/// React Router `<Routes>` trees (see the module doc).
fn scan_jsx_routes(source: &str) -> Scan {
    let b = source.as_bytes();
    let mut scan = Scan::default();
    // Composed path of every open (non self-closing) `<Route>`.
    let mut open: Vec<String> = Vec::new();
    let mut resume = 0usize;
    for (lt, _) in source.match_indices('<') {
        if lt < resume {
            continue;
        }
        let after = source.get(lt + 1..).unwrap_or("");
        let word_end = |rest: &str, word: &str| {
            rest.strip_prefix(word)
                .is_some_and(|r| r.as_bytes().first().is_none_or(|&c| !is_ident_byte(c)))
        };
        if word_end(after, "/Route") {
            open.pop();
            continue;
        }
        if !word_end(after, "Route") {
            continue;
        }
        // `Array<Route>` / `useState<Route>`: a TS generic, not JSX.
        if lt > 0 && is_ident_byte(b[lt - 1]) {
            continue;
        }
        let Some(tag) = parse_tag(source, lt) else {
            continue;
        };
        resume = tag.end;
        let parent = open.last().cloned();
        let parent_path = parent.as_deref().unwrap_or("");
        let path_lit = tag.attr("path").and_then(attr_literal);
        let index = match tag.attr("index") {
            None => tag.has("index"),
            Some(v) => attr_expr(v) == "true",
        };
        let own = match (&path_lit, index) {
            (Some(p), _) => {
                let composed = compose(parent_path, p);
                if !is_route_path(&composed) {
                    scan.rejected += 1;
                    None
                } else {
                    if parent.is_some() && !p.starts_with('/') {
                        scan.children += 1;
                    }
                    Some(composed)
                }
            }
            (None, true) => {
                if parent.is_some() {
                    scan.children += 1;
                }
                Some(normalize(parent_path))
            }
            (None, false) => None,
        };
        if let Some(path) = &own {
            let mut rec = RouteRecord {
                path: path.clone(),
                catchall: is_catchall(path),
                line: line_of(source, lt),
                ..RouteRecord::default()
            };
            match tag.attr("element").and_then(|v| jsx_element(attr_expr(v))) {
                Some(Element::Handler(h)) => rec.handler = Some(h),
                Some(Element::Redirect(t)) => rec.redirect = absolute_link(path, &t),
                None => {}
            }
            if rec.handler.is_none() {
                rec.handler = ["Component", "component"]
                    .iter()
                    .find_map(|k| tag.attr(k).and_then(|v| ident_path(attr_expr(v))))
                    .or_else(|| {
                        tag.attr("lazy")
                            .and_then(|v| lazy_import_handler(attr_expr(v)))
                    });
            }
            scan.records.push(rec);
        }
        if !tag.self_closing {
            open.push(own.unwrap_or_else(|| normalize(parent_path)));
        }
    }
    scan
}

// ----------------------------------------------------------------------------
// Object-literal tables
// ----------------------------------------------------------------------------

/// A top-level key of an object literal and its value's byte span.
#[derive(Debug, Clone)]
struct Key {
    name: String,
    start: usize,
    end: usize,
}

/// An object literal that has a `path` or `index` key: its `{`..`}` span
/// (the index of each brace) and its top-level keys.
#[derive(Debug)]
struct Obj {
    open: usize,
    close: usize,
    keys: Vec<Key>,
}

impl Obj {
    fn key(&self, name: &str) -> Option<&Key> {
        self.keys.iter().find(|k| k.name == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    Brace,
    Bracket,
    Paren,
    /// Inside a backtick literal's text.
    Template,
    /// Inside a backtick literal's `${ ... }`.
    TemplateExpr,
}

struct Frame {
    open: Open,
    at: usize,
    keys: Vec<Key>,
    /// The next identifier / quoted string followed by `:` is a key.
    expect_key: bool,
    /// The last key's value is still open (ends at the next `,` or `}`).
    pending: bool,
}

impl Frame {
    fn new(open: Open, at: usize) -> Self {
        Frame {
            open,
            at,
            keys: Vec::new(),
            expect_key: open == Open::Brace,
            pending: false,
        }
    }

    fn end_value(&mut self, at: usize) {
        if self.pending {
            if let Some(k) = self.keys.last_mut() {
                k.end = at;
            }
            self.pending = false;
        }
    }
}

/// A `/` after this byte starts a regex literal rather than a division.
fn regex_may_follow(prev: u8) -> bool {
    matches!(
        prev,
        b'(' | b','
            | b'='
            | b':'
            | b'['
            | b'!'
            | b'&'
            | b'|'
            | b'?'
            | b'{'
            | b';'
            | b'+'
            | b'-'
            | b'*'
            | b'%'
            | b'~'
            | b'^'
            | 0
    )
}

/// Keywords after which a `/` starts a regex literal.
fn keyword_before_regex(word: &str) -> bool {
    matches!(
        word,
        "return"
            | "typeof"
            | "case"
            | "in"
            | "of"
            | "new"
            | "delete"
            | "void"
            | "throw"
            | "yield"
            | "await"
    )
}

/// Index just past a regex literal whose `/` is at `i`, or `i + 1` when the
/// line ends first (then it was a division after all).
fn skip_regex(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    let mut class = false;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'\n' => return i + 1,
            b'[' => {
                class = true;
                j += 1;
            }
            b']' => {
                class = false;
                j += 1;
            }
            b'/' if !class => return j + 1,
            _ => j += 1,
        }
    }
    i + 1
}

/// Pop the frame a closer `c` matches (see [`lex_objects`]); an object with a
/// `path` / `index` key is pushed onto `out`.
fn close_frame(stack: &mut Vec<Frame>, c: u8, at: usize, out: &mut Vec<Obj>) {
    let closes = |f: &Frame| match c {
        b'}' => matches!(f.open, Open::Brace | Open::TemplateExpr),
        b']' => f.open == Open::Bracket,
        _ => f.open == Open::Paren,
    };
    // `)` / `]` never close across a brace or template; `}` closes across
    // unbalanced `(` / `[` (discarding them) but never across a template.
    let crossable = |f: &Frame| match c {
        b'}' => matches!(f.open, Open::Bracket | Open::Paren),
        _ => false,
    };
    let mut idx = None;
    for (k, f) in stack.iter().enumerate().rev() {
        if closes(f) {
            idx = Some(k);
            break;
        }
        if !crossable(f) {
            break;
        }
    }
    let Some(k) = idx else {
        return;
    };
    stack.truncate(k + 1);
    let Some(mut f) = stack.pop() else {
        return;
    };
    if f.open != Open::Brace {
        return;
    }
    f.end_value(at);
    if f.keys.iter().any(|k| k.name == "path" || k.name == "index") {
        out.push(Obj {
            open: f.at,
            close: at,
            keys: f.keys,
        });
    }
}

/// One forward pass: every object literal holding a `path` or `index` key,
/// with its top-level keys (see the module doc).
fn lex_objects(src: &str) -> Vec<Obj> {
    let b = src.as_bytes();
    let mut stack: Vec<Frame> = Vec::new();
    let mut out: Vec<Obj> = Vec::new();
    // Last significant code byte; 0 = start of input (a regex may follow).
    let mut prev: u8 = 0;
    let mut i = 0usize;
    while i < b.len() {
        if stack.last().is_some_and(|f| f.open == Open::Template) {
            match b[i] {
                b'\\' => i += 2,
                b'`' => {
                    stack.pop();
                    prev = b'`';
                    i += 1;
                }
                b'$' if b.get(i + 1) == Some(&b'{') => {
                    stack.push(Frame::new(Open::TemplateExpr, i + 1));
                    prev = b'{';
                    i += 2;
                }
                _ => i += 1,
            }
            continue;
        }
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' {
            match b.get(i + 1) {
                Some(b'/') => {
                    i = src
                        .get(i..)
                        .and_then(|s| s.find('\n'))
                        .map_or(b.len(), |n| i + n);
                    continue;
                }
                Some(b'*') => {
                    i = src
                        .get(i + 2..)
                        .and_then(|s| s.find("*/"))
                        .map_or(b.len(), |n| i + 2 + n + 2);
                    continue;
                }
                _ if regex_may_follow(prev) => {
                    let end = skip_regex(b, i);
                    if end > i + 1 {
                        if let Some(f) = stack.last_mut() {
                            f.expect_key = false;
                        }
                        prev = b'/';
                        i = end;
                        continue;
                    }
                }
                _ => {}
            }
        }
        match c {
            b'\'' | b'"' => {
                let end = skip_quoted(b, i);
                let closed = end > i + 1 && b.get(end - 1) == Some(&c);
                if let Some(f) = stack.last_mut()
                    && f.open == Open::Brace
                {
                    if f.expect_key && closed {
                        let mut j = end;
                        while j < b.len() && b[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        if b.get(j) == Some(&b':')
                            && let Some(name) = src.get(i + 1..end - 1)
                        {
                            f.keys.push(Key {
                                name: name.to_string(),
                                start: j + 1,
                                end: j + 1,
                            });
                            f.pending = true;
                            f.expect_key = false;
                            prev = b':';
                            i = j + 1;
                            continue;
                        }
                    }
                    f.expect_key = false;
                }
                prev = c;
                i = end;
            }
            b'`' => {
                if let Some(f) = stack.last_mut() {
                    f.expect_key = false;
                }
                stack.push(Frame::new(Open::Template, i));
                i += 1;
            }
            b'{' | b'[' | b'(' => {
                if let Some(f) = stack.last_mut() {
                    f.expect_key = false;
                }
                let open = match c {
                    b'{' => Open::Brace,
                    b'[' => Open::Bracket,
                    _ => Open::Paren,
                };
                stack.push(Frame::new(open, i));
                prev = c;
                i += 1;
            }
            b'}' | b']' | b')' => {
                close_frame(&mut stack, c, i, &mut out);
                prev = c;
                i += 1;
            }
            b',' => {
                if let Some(f) = stack.last_mut()
                    && f.open == Open::Brace
                {
                    f.end_value(i);
                    f.expect_key = true;
                }
                prev = c;
                i += 1;
            }
            c if is_ident_byte(c) => {
                let s = i;
                while i < b.len() && is_ident_byte(b[i]) {
                    i += 1;
                }
                let word = src.get(s..i).unwrap_or("");
                prev = if keyword_before_regex(word) {
                    b'('
                } else {
                    b'a'
                };
                if let Some(f) = stack.last_mut()
                    && f.open == Open::Brace
                {
                    if f.expect_key {
                        let mut j = i;
                        while j < b.len() && b[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        if b.get(j) == Some(&b':') {
                            f.keys.push(Key {
                                name: word.to_string(),
                                start: j + 1,
                                end: j + 1,
                            });
                            f.pending = true;
                            prev = b':';
                            i = j + 1;
                        }
                    }
                    f.expect_key = false;
                }
            }
            _ => {
                if let Some(f) = stack.last_mut()
                    && f.open == Open::Brace
                {
                    f.expect_key = false;
                }
                prev = c;
                i += 1;
            }
        }
    }
    out
}

/// A route record found in an object table, before composition.
struct Found {
    open: usize,
    close: usize,
    /// The `children` value span, when the record has one.
    children: Option<(usize, usize)>,
    /// `Some(path)` for a `path` record, `None` for an `index: true` one.
    path: Option<String>,
    handler: Option<String>,
    /// Raw `redirectTo` / `redirect` target (relative to the PARENT path).
    redirect_rel_parent: Option<String>,
    /// Raw `<Navigate to>` target (relative to the route's OWN path).
    redirect_rel_own: Option<String>,
}

/// Object-literal route tables (see the module doc).
fn scan_route_records(source: &str) -> Scan {
    let mut scan = Scan::default();
    let mut objs = lex_objects(source);
    objs.sort_by_key(|o| o.open);
    let value = |k: &Key| source.get(k.start..k.end).unwrap_or("");

    let mut found: Vec<Found> = Vec::new();
    for o in &objs {
        let has_router_key = o
            .keys
            .iter()
            .any(|k| ROUTER_KEYS.contains(&k.name.as_str()));
        let path = match o.key("path") {
            Some(k) => match literal(clip(value(k), MAX_VALUE)) {
                Some(p) => {
                    if !has_router_key || p.contains("${") {
                        scan.rejected += 1;
                        continue;
                    }
                    Some(p)
                }
                // A computed path (`path: base + '/x'`) is not a table entry.
                None => continue,
            },
            None => {
                // React Router `{ index: true, element }`: the parent's page.
                let index = o.key("index").is_some_and(|k| value(k).trim() == "true");
                let renders = ["element", "Component", "component", "lazy"]
                    .iter()
                    .any(|n| o.key(n).is_some());
                if !(index && renders) {
                    continue;
                }
                None
            }
        };
        let mut f = Found {
            open: o.open,
            close: o.close,
            children: o.key("children").map(|k| (k.start, k.end)),
            path,
            handler: None,
            redirect_rel_parent: None,
            redirect_rel_own: None,
        };
        for k in &o.keys {
            let v = clip(value(k), MAX_VALUE);
            match k.name.as_str() {
                "component" | "Component" if f.handler.is_none() => {
                    f.handler = ident_path(v).or_else(|| {
                        v.contains("import(")
                            .then(|| lazy_import_handler(v))
                            .flatten()
                    });
                }
                "loadComponent" | "lazy" if f.handler.is_none() => {
                    f.handler = lazy_import_handler(v);
                }
                "element" => match jsx_element(v) {
                    Some(Element::Handler(h)) if f.handler.is_none() => f.handler = Some(h),
                    Some(Element::Redirect(t)) => f.redirect_rel_own = Some(t),
                    _ => {}
                },
                "redirectTo" | "redirect" => {
                    if let Some(t) = literal(v) {
                        f.redirect_rel_parent = Some(t);
                    }
                }
                _ => {}
            }
        }
        found.push(f);
    }

    // Compose in source order: a parent always opens before its children, so
    // its path is known when a child is reached. `enclosing` holds the records
    // whose span is still open at the current record.
    let mut composed: Vec<Option<String>> = Vec::with_capacity(found.len());
    let mut enclosing: Vec<usize> = Vec::new();
    for (at, f) in found.iter().enumerate() {
        while enclosing.last().is_some_and(|&p| found[p].close < f.open) {
            enclosing.pop();
        }
        let parent = enclosing.iter().rev().copied().find(|&p| {
            found[p]
                .children
                .is_some_and(|(s, e)| s <= f.open && f.close < e)
                && composed[p].is_some()
        });
        enclosing.push(at);
        let parent_path = parent.and_then(|p| composed[p].clone());
        let base = parent_path.as_deref().unwrap_or("");
        let own = match &f.path {
            Some(p) => compose(base, p),
            None => normalize(base),
        };
        if !is_route_path(&own) {
            scan.rejected += 1;
            composed.push(None);
            continue;
        }
        if parent_path.is_some() && !f.path.as_deref().is_some_and(|p| p.starts_with('/')) {
            scan.children += 1;
        }
        let redirect = f
            .redirect_rel_parent
            .as_deref()
            .and_then(|t| absolute_link(base, t))
            .or_else(|| {
                f.redirect_rel_own
                    .as_deref()
                    .and_then(|t| absolute_link(&own, t))
            });
        scan.records.push(RouteRecord {
            catchall: is_catchall(&own),
            path: own.clone(),
            handler: f.handler.clone(),
            redirect,
            line: line_of(source, f.open),
        });
        composed.push(Some(own));
    }
    scan
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

    fn paths(out: &NavRouteOut) -> Vec<String> {
        out.nodes
            .iter()
            .filter_map(|n| out.nav.name_by_id.get(&n.id).cloned())
            .collect()
    }

    fn route_id(path: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, &nav_route_qname(path))
    }

    /// `(route path, qualifier text, category)` for every ref, in order.
    fn refs(out: &NavRouteOut) -> Vec<(String, String, &'static str)> {
        out.refs
            .iter()
            .map(|r| {
                let path = out.nav.name_by_id.get(&r.from).cloned().unwrap_or_default();
                let q = match &r.qualifier {
                    CallQualifier::Bare(n) => n.clone(),
                    CallQualifier::Attribute { base, name } => format!("{base}.{name}"),
                    other => format!("{other:?}"),
                };
                let cat = if r.category == edge_category::HANDLED_BY {
                    "HANDLED_BY"
                } else if r.category == edge_category::NAVIGATES_TO {
                    "NAVIGATES_TO"
                } else {
                    "?"
                };
                (path, q, cat)
            })
            .collect()
    }

    fn has_ref(out: &NavRouteOut, path: &str, q: &str, cat: &str) -> bool {
        refs(out)
            .iter()
            .any(|(p, qq, c)| p == path && qq == q && *c == cat)
    }

    const ANGULAR: &str = r#"
import { Routes } from '@angular/router';
import { AdminShellComponent } from './admin-shell.component';

export const routes: Routes = [
  { path: 'admin', component: AdminShellComponent, children: [
      { path: 'users', component: AdminUsersComponent },
      { path: '', component: AdminHomeComponent },
      { path: '/abs', component: AbsComponent },
  ] },
  { path: 'home', component: HomeComponent },
  { path: 'chat', redirectTo: '/home', pathMatch: 'full' },
  { path: 'old', redirectTo: 'home' },
  { path: 'profile', loadComponent: () => import('./profile.component').then(m => m.ProfileComponent) },
  { path: 'lazy', loadComponent: () => import('./user-list.component') },
  { path: '**', redirectTo: '/login' },
];
"#;

    #[test]
    fn angular_child_composition() {
        let out = extract_route_tables(ANGULAR, module_id(), repo());
        let p = paths(&out);
        assert!(p.contains(&"/admin/users".to_string()), "{p:?}");
        assert!(
            !p.contains(&"/users".to_string()),
            "un-composed child leaked: {p:?}"
        );
        // An absolute child path stands alone.
        assert!(p.contains(&"/abs".to_string()), "{p:?}");
        assert!(!p.contains(&"/admin/abs".to_string()), "{p:?}");
        // `users` and `''` compose; the absolute `/abs` does not.
        assert_eq!(out.children, 2);
        assert!(has_ref(
            &out,
            "/admin/users",
            "AdminUsersComponent",
            "HANDLED_BY"
        ));
        assert!(has_ref(&out, "/admin", "AdminShellComponent", "HANDLED_BY"));
    }

    #[test]
    fn empty_child_path_takes_parent_path() {
        let out = extract_route_tables(ANGULAR, module_id(), repo());
        // `/admin` is one node carrying both the shell and the '' child's page.
        assert_eq!(paths(&out).iter().filter(|p| *p == "/admin").count(), 1);
        assert!(has_ref(&out, "/admin", "AdminHomeComponent", "HANDLED_BY"));
    }

    #[test]
    fn load_component_handler() {
        let out = extract_route_tables(ANGULAR, module_id(), repo());
        assert!(has_ref(&out, "/profile", "ProfileComponent", "HANDLED_BY"));
        // No `.then`: the Angular CLI file-name convention names the class.
        assert!(has_ref(&out, "/lazy", "UserListComponent", "HANDLED_BY"));
    }

    #[test]
    fn redirect_relative_and_absolute() {
        let out = extract_route_tables(ANGULAR, module_id(), repo());
        assert!(has_ref(&out, "/chat", "/home", "NAVIGATES_TO"));
        // Relative `redirectTo: 'home'` resolves against the parent (root).
        assert!(has_ref(&out, "/old", "/home", "NAVIGATES_TO"));
        let nested = r#"
const routes = [
  { path: 'admin', children: [ { path: '', redirectTo: 'users', pathMatch: 'full' },
                               { path: 'users', component: Users } ] },
];
"#;
        let out = extract_route_tables(nested, module_id(), repo());
        assert!(
            has_ref(&out, "/admin", "/admin/users", "NAVIGATES_TO"),
            "{:?}",
            refs(&out)
        );
        // The ref's `from` is the ROUTE itself (never lifted).
        let r = out
            .refs
            .iter()
            .find(|r| r.category == edge_category::NAVIGATES_TO)
            .expect("redirect ref");
        assert_eq!(r.from, route_id("/admin"));
        assert_eq!(r.from_module, module_id());
    }

    #[test]
    fn catchall_route() {
        let out = extract_route_tables(ANGULAR, module_id(), repo());
        assert!(paths(&out).contains(&"/**".to_string()));
        assert!(has_ref(&out, "/**", "/login", "NAVIGATES_TO"));
        assert_eq!(out.catchalls, 1);
        let vue = "const routes = [{ path: '/:pathMatch(.*)*', component: NotFound }];";
        let out = extract_route_tables(vue, module_id(), repo());
        assert_eq!(paths(&out), ["/:pathMatch(.*)*"]);
        assert_eq!(out.catchalls, 1);
    }

    #[test]
    fn rejects_non_router_objects() {
        let src = r#"
export const exportCfg = { path: 'tmp/out.txt', mode: 1 };
server.route({ method: 'GET', path: '/users', handler: listUsers });
User.find().populate({ path: 'author', select: 'name' });
"#;
        let out = extract_route_tables(src, module_id(), repo());
        assert!(out.nodes.is_empty(), "{:?}", paths(&out));
        assert!(out.refs.is_empty());
        assert_eq!(out.rejected, 3);
    }

    #[test]
    fn mongoose_index_option_is_not_a_route() {
        let src = "const s = new Schema({ email: { type: String, index: true, unique: true } });";
        let out = extract_route_tables(src, module_id(), repo());
        assert!(out.nodes.is_empty());
        assert_eq!(out.rejected, 0);
    }

    #[test]
    fn react_nested_jsx_index_and_navigate() {
        let src = r#"
import { Routes, Route, Navigate } from 'react-router-dom';
export function App() {
  return (
    <Routes>
      <Route path="/" element={<Home />} />
      <Route path="/settings" element={<Settings />}>
        <Route path="profile" element={<ProfilePage />} />
        <Route index element={<Settings />} />
      </Route>
      <Route path="/old" element={<Navigate to="/reports" />} />
      <Route path="/lazy" element={<Suspense fallback={<Spinner />}><Reports /></Suspense>} />
    </Routes>
  );
}
"#;
        let out = extract_route_tables(src, module_id(), repo());
        let p = paths(&out);
        assert_eq!(p, ["/", "/settings", "/settings/profile", "/old", "/lazy"]);
        assert!(!p.contains(&"/profile".to_string()));
        assert!(has_ref(
            &out,
            "/settings/profile",
            "ProfilePage",
            "HANDLED_BY"
        ));
        assert!(has_ref(&out, "/old", "/reports", "NAVIGATES_TO"));
        // Suspense is looked through; its fallback is not the page.
        assert!(has_ref(&out, "/lazy", "Reports", "HANDLED_BY"));
        assert!(!has_ref(&out, "/lazy", "Spinner", "HANDLED_BY"));
        // The index route repeats its parent's element: one HANDLED_BY.
        let settings = refs(&out)
            .into_iter()
            .filter(|(p, _, c)| p == "/settings" && *c == "HANDLED_BY")
            .count();
        assert_eq!(settings, 1);
        // `profile` and the index route compose onto `/settings`.
        assert_eq!(out.children, 2);
        assert_eq!(out.redirects, 1);
    }

    #[test]
    fn react_object_form_component_and_lazy() {
        let src = r#"
export const router = createBrowserRouter([
  { path: '/reports', Component: Reports },
  { path: '/admin', element: <AdminLayout />, children: [
      { index: true, element: <AdminHome /> },
      { path: 'audit', lazy: () => import('./pages/Audit') },
  ] },
  { path: '/gone', element: <Navigate to="../reports" replace /> },
]);
"#;
        let out = extract_route_tables(src, module_id(), repo());
        assert_eq!(paths(&out), ["/reports", "/admin", "/admin/audit", "/gone"]);
        assert!(has_ref(&out, "/reports", "Reports", "HANDLED_BY"));
        assert!(has_ref(&out, "/admin", "AdminHome", "HANDLED_BY"));
        assert!(has_ref(&out, "/admin/audit", "Audit", "HANDLED_BY"));
        // `<Navigate to>` is relative to the route's own path.
        assert!(
            has_ref(&out, "/gone", "/reports", "NAVIGATES_TO"),
            "{:?}",
            refs(&out)
        );
    }

    #[test]
    fn vue_lazy_import_component() {
        let src = r#"
const routes = [
  { path: '/', component: () => import('./views/Home.vue') },
  { path: '/users', component: Users, children: [
      { path: ':id', component: () => import('./views/UserDetail.vue') },
  ] },
  { path: '/old', redirect: '/users' },
];
"#;
        let out = extract_route_tables(src, module_id(), repo());
        assert_eq!(paths(&out), ["/", "/users", "/users/:id", "/old"]);
        assert!(has_ref(&out, "/", "Home", "HANDLED_BY"));
        assert!(has_ref(&out, "/users/:id", "UserDetail", "HANDLED_BY"));
        assert!(has_ref(&out, "/old", "/users", "NAVIGATES_TO"));
    }

    #[test]
    fn one_literal_in_plain_ts_emits_one_node() {
        let src = "export const routes = [{ path: 'home', component: HomeComponent }];";
        let out = extract_route_tables(src, module_id(), repo());
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(out.nav_routes, 1);
        let node = &out.nodes[0];
        assert_eq!(node.id, route_id("/home"));
        assert_eq!(
            out.nav.qname_by_id.get(&node.id).map(String::as_str),
            Some("page:/home")
        );
        assert_eq!(
            node.cells.len(),
            2,
            "one ROUTE_METHOD + one ORIGIN, not duplicates"
        );
    }

    #[test]
    fn lexer_survives_strings_comments_regex_and_templates() {
        let src = r#"
// { path: 'commented', component: X }
/* { path: 'block', component: Y } */
const re = /[{}'"]/g;
const s = "{ path: 'in-string', component: Z }";
const t = `${ { a: 1 }.a } { path: 'in-template', component: W }`;
const routes = [{ path: 'real', component: Real, title: `x${y}z` }];
"#;
        let out = extract_route_tables(src, module_id(), repo());
        assert_eq!(paths(&out), ["/real"]);
        assert!(has_ref(&out, "/real", "Real", "HANDLED_BY"));
    }

    #[test]
    fn multibyte_source_never_panics() {
        let src = "const r = [{ path: 'café', component: Ünï, t: 'ü\\'' }, { path: '日本', children: [{ path: 'ä', component: X }] }]; <Route path=\"é\" element={<Ö />} />";
        let out = extract_route_tables(src, module_id(), repo());
        assert!(
            paths(&out).contains(&"/日本/ä".to_string()),
            "{:?}",
            paths(&out)
        );
        for cut in 0..src.len() {
            if src.is_char_boundary(cut) {
                let _ = extract_route_tables(&src[..cut], module_id(), repo());
            }
        }
    }

    #[test]
    fn generic_route_type_is_not_a_jsx_tag() {
        let src = r#"
const saved: Array<Route> = [];
<Routes><Route path="/a" element={<A />} /></Routes>
"#;
        let out = extract_route_tables(src, module_id(), repo());
        assert_eq!(paths(&out), ["/a"]);
    }

    #[test]
    fn stem_names() {
        assert_eq!(stem_name("./views/Home.vue").as_deref(), Some("Home"));
        assert_eq!(stem_name("./pages/Reports").as_deref(), Some("Reports"));
        assert_eq!(stem_name("./pages/users/index").as_deref(), Some("users"));
        assert_eq!(
            stem_name("./user-list.component").as_deref(),
            Some("UserListComponent")
        );
    }

    #[test]
    fn emit_is_public_for_grafts() {
        let recs = vec![RouteRecord {
            path: "/docs/:slug*".to_string(),
            handler: Some("DocPage".to_string()),
            redirect: None,
            catchall: true,
            line: 7,
        }];
        let out = emit_nav_routes(&recs, module_id(), repo());
        assert_eq!(out.nav_routes, 1);
        assert_eq!(out.catchalls, 1);
        assert_eq!(out.bound, 1);
        assert_eq!(out.refs[0].from, route_id("/docs/:slug*"));
        assert_eq!(out.refs[0].line, 7, "the record's row is the ref's site line");
    }
}
