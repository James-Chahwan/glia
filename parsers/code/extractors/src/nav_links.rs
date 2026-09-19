//! Navigation link sites (LA.6c): the places a frontend sends the user to
//! another page, emitted as `NAVIGATES_TO` refs that LA.6a's resolver
//! (graph/src/nav.rs) binds against the route tables LA.6b mints.
//!
//! # The link contract (graph/src/nav.rs module doc)
//!
//! One ref per distinct link per file: `UnresolvedRef { from: module_id,
//! from_module: module_id, qualifier: Bare(link), category: NAVIGATES_TO }`.
//! `link` is `/path` (router tier: can be reported dead) or `href:/path`
//! (plain-anchor tier: never dead). Query and fragment are stripped, and every
//! segment that is not literal text is written `${...}`. A relative link (no
//! leading `/`) depends on the current route, so it is skipped here and
//! counted `dynamic_skipped` with every other non-literal target.
//!
//! # Router tier
//!
//! - Angular: `.navigate([...])` (a command array: `['/user', id]` gives
//!   `/user/${...}`), `.navigateByUrl('...')`, `routerLink="/x"` and
//!   `[routerLink]="..."` (a literal, an array, or every operand of a ternary /
//!   `||` / `??`), in inline templates and in `.component.html`
//!   ([`extract_template_links`]).
//! - React Router / Next: `<Link to>`, `<NavLink to>`, `<Navigate to>` (except
//!   inside a route `element`, which LA.6b already emits as the route's
//!   redirect), `<Link href>` when the file imports `next/link`, the function
//!   `useNavigate()` returns, `router.push` / `router.replace` (and
//!   `history.push` under react-router), `redirect(...)` from `next/navigation`.
//! - Vue: `<router-link to>` / `<RouterLink to>` and their `:to` bindings,
//!   `router.push(...)` / `this.$router.push(...)` with a string or
//!   `{ path: '...' }`. A `{ name: '...' }` push is a named route, outside the
//!   table's path space: dynamic.
//! - Origin share links: a template literal that starts
//!   `${window.location.origin}/...`, or `location.origin + '/...'`, is a deep
//!   link into this app (the quokka `/connect?ref=` share link). Counted in
//!   `origin` as well. One passed straight to an HTTP call (`fetch(...)`,
//!   `.get(...)`) is a request, not a link, and is left alone.
//!
//! # Plain tier
//!
//! `href` on any tag but `<base>` / `<link>` (and a next/link `<Link>`, which
//! is router tier), `window.location.href = '/x'`, `location.assign('/x')`,
//! `location.replace('/x')`.
//!
//! # Never a link
//!
//! Asset paths ([`is_asset_path`]: `/favicon.ico`, `/quokka_nobg.png`),
//! protocol-relative `//x`, any `scheme:` URL (another host, `mailto:`,
//! `tel:`, `javascript:`), fragments `#x`.
//!
//! # Gate and robustness
//!
//! [`extract_nav_links`] returns empty unless the source names a router (see
//! `ROUTER_NEEDLES`) or `location.origin`, so a backend `.ts` costs one
//! `contains` pass per needle. Comments are blanked first (`//`, `/* */`,
//! `<!-- -->`), so commented-out navigation is not a link. Every argument read
//! is capped at [`MAX_ARG`] bytes and every tag at [`MAX_TAG`]; literal
//! skipping is depth-bounded; every slice goes through `str::get`.

use std::borrow::Cow;
use std::collections::HashSet;

use repo_graph_code_domain::{CallQualifier, UnresolvedRef, edge_category};
use repo_graph_core::NodeId;

/// Byte cap on one argument / assigned expression.
const MAX_ARG: usize = 1024;
/// Byte cap on one opening tag.
const MAX_TAG: usize = 4096;
/// Nesting cap on template literals inside `${...}` inside template literals.
const MAX_NEST: usize = 32;

/// A source that names none of these (and no `location.origin`) holds no
/// router link. `routerLink` covers NgModule components, whose templates use
/// the directive without importing `@angular/router`.
const ROUTER_NEEDLES: &[&str] = &[
    "@angular/router",
    "routerLink",
    "react-router",
    "next/link",
    "next/router",
    "next/navigation",
    "vue-router",
    "$router",
    "<router-link",
    "<RouterLink",
];

/// Callees whose first argument is a request URL, never a page link.
const HTTP_CALLEES: &[&str] = &[
    "fetch",
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "head",
    "request",
    "axios",
    "ky",
    "WebSocket",
    "EventSource",
    "sendBeacon",
];

/// A file's navigation refs plus the counters behind the engine's
/// `[nav-links]` marker.
#[derive(Debug, Default)]
pub struct NavLinkOut {
    pub refs: Vec<UnresolvedRef>,
    /// Router-tier refs (`/path`), origin share links included.
    pub router: usize,
    /// Plain-tier refs (`href:/path`).
    pub href: usize,
    /// Router-tier refs built from `location.origin`.
    pub origin: usize,
    /// Refs read from an Angular `.component.html` template.
    pub template: usize,
    /// Link sites whose target is not a literal absolute path (a variable, a
    /// relative command, a named route): nothing emitted for them.
    pub dynamic_skipped: usize,
}

/// Every navigation link in one TS-family file (`.ts` / `.tsx` / `.js` /
/// `.jsx` / `.vue`, whole source). `lang` is the engine's routing tag: an
/// `angular` file is scanned for the Angular forms only, since JSX and Vue
/// link components never appear in one.
pub fn extract_nav_links(source: &str, lang: &str, module_id: NodeId) -> NavLinkOut {
    let routed = ROUTER_NEEDLES.iter().any(|n| source.contains(n));
    let origin = source.contains("location.origin");
    if !routed && !origin {
        return NavLinkOut::default();
    }
    let text = mask_comments(source, true);
    let mut em = Emitter::new(module_id);
    if routed {
        let ctx = Ctx::of(&text, lang);
        scan_calls(&text, &ctx, &mut em);
        scan_tags(&text, &ctx, &mut em);
        scan_location(&text, &mut em);
    }
    if origin {
        scan_origin(&text, &mut em);
    }
    em.out
}

/// Every navigation link in an Angular `.component.html` template: the
/// `routerLink` forms, `href`, and `(click)` handlers calling
/// `.navigate(...)`. `module_id` is the component's `.component.ts` module
/// (both files share one MODULE qname), so the refs resolve and lift exactly
/// like the component file's own.
pub fn extract_template_links(source: &str, module_id: NodeId) -> NavLinkOut {
    let text = mask_comments(source, false);
    let mut em = Emitter::new(module_id);
    let ctx = Ctx::default();
    scan_calls(&text, &ctx, &mut em);
    scan_tags(&text, &ctx, &mut em);
    em.out.template = em.out.refs.len();
    em.out
}

/// The last path segment carries a file extension: a static asset the server
/// hands out (`/favicon.ico`, `/manifest.webmanifest`, `/docs/guide.pdf`),
/// never a page. A `${...}` segment is never an asset.
pub fn is_asset_path(path: &str) -> bool {
    is_asset_segment(path.rsplit('/').next().unwrap_or(""))
}

/// One raw segment ends in a file extension. A dynamic segment counts by the
/// text after its last `}` (`${id}.pdf` is an asset, `${...}` is not).
fn is_asset_segment(seg: &str) -> bool {
    let (tail, dynamic) = match seg.rfind('}') {
        Some(k) => (seg.get(k + 1..).unwrap_or(""), true),
        None => (seg, false),
    };
    match tail.rsplit_once('.') {
        Some((stem, ext)) => {
            (dynamic || !stem.is_empty())
                && (1..=16).contains(&ext.len())
                && ext.bytes().all(|c| c.is_ascii_alphanumeric())
                && ext.bytes().any(|c| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

// ----------------------------------------------------------------------------
// Emission
// ----------------------------------------------------------------------------

/// What one link expression points at.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// A normalised absolute path (`/user/${...}`).
    Link(String),
    /// Not literal, or relative to the current route.
    Dynamic,
    /// Never a page link (asset, another host, `mailto:`, a fragment, `null`).
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Router,
    Href,
}

struct Emitter {
    module_id: NodeId,
    seen: HashSet<String>,
    out: NavLinkOut,
}

impl Emitter {
    fn new(module_id: NodeId) -> Self {
        Self {
            module_id,
            seen: HashSet::new(),
            out: NavLinkOut::default(),
        }
    }

    /// One link site: each `Link` target becomes a ref (once per file); a site
    /// with no link and a non-literal target counts as dynamic.
    fn site(&mut self, targets: Vec<Target>, tier: Tier, origin: bool) {
        let mut linked = false;
        let mut dynamic = false;
        for t in targets {
            match t {
                Target::Link(path) => {
                    linked = true;
                    self.emit(path, tier, origin);
                }
                Target::Dynamic => dynamic = true,
                Target::Skip => {}
            }
        }
        if dynamic && !linked {
            self.out.dynamic_skipped += 1;
        }
    }

    fn emit(&mut self, path: String, tier: Tier, origin: bool) {
        let link = match tier {
            Tier::Router => path,
            Tier::Href => format!("href:{path}"),
        };
        if !self.seen.insert(link.clone()) {
            return;
        }
        match tier {
            Tier::Router => {
                self.out.router += 1;
                if origin {
                    self.out.origin += 1;
                }
            }
            Tier::Href => self.out.href += 1,
        }
        self.out.refs.push(UnresolvedRef {
            from: self.module_id,
            from_module: self.module_id,
            qualifier: CallQualifier::Bare(link),
            category: edge_category::NAVIGATES_TO,
        });
    }
}

// ----------------------------------------------------------------------------
// Link expressions
// ----------------------------------------------------------------------------

/// A raw link text (`/user/${id}?tab=1`, `/user/{{ u.id }}`) as a target:
/// query and fragment stripped, `.` / `..` / empty segments folded, every
/// segment holding `${` or `{{` written `${...}`.
fn path_target(raw: &str) -> Target {
    let t = raw.trim();
    if t.is_empty() {
        return Target::Dynamic;
    }
    if t.starts_with('#') || t.starts_with("//") || has_scheme(t) {
        return Target::Skip;
    }
    if !t.starts_with('/') {
        return Target::Dynamic;
    }
    let mut raw: Vec<&str> = Vec::new();
    for seg in split_path(t) {
        match seg {
            "" | "." => {}
            ".." => {
                raw.pop();
            }
            s => raw.push(s),
        }
    }
    if raw.last().is_some_and(|s| is_asset_segment(s)) {
        return Target::Skip;
    }
    let segs: Vec<&str> = raw
        .iter()
        .map(|s| {
            if s.contains("${") || s.contains("{{") {
                "${...}"
            } else {
                s
            }
        })
        .collect();
    Target::Link(format!("/{}", segs.join("/")))
}

/// `scheme:` before any `/`: `https://x`, `mailto:a`, `tel:1`, `javascript:`.
fn has_scheme(t: &str) -> bool {
    let Some(colon) = t.find(':') else {
        return false;
    };
    let scheme = t.get(..colon).unwrap_or("");
    !scheme.is_empty()
        && !scheme.contains('/')
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
}

/// `t` split on `/` outside `${...}` / `{{...}}`, ending at the first `?` or
/// `#` outside them.
fn split_path(t: &str) -> Vec<&str> {
    let b = t.as_bytes();
    let mut segs = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut end = b.len();
    for (j, &c) in b.iter().enumerate() {
        match c {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'/' if depth == 0 => {
                segs.push(t.get(start..j).unwrap_or(""));
                start = j + 1;
            }
            b'?' | b'#' if depth == 0 => {
                end = j;
                break;
            }
            _ => {}
        }
    }
    segs.push(t.get(start..end.max(start)).unwrap_or(""));
    segs
}

/// Every target a link expression can take: both arms of a ternary (the
/// condition is not a link), every operand of `||` / `??`, else the one
/// expression.
fn targets_of(expr: &str) -> Vec<Target> {
    targets_at(expr, 0)
}

fn targets_at(expr: &str, depth: usize) -> Vec<Target> {
    let e = expr.trim();
    if depth < 16 {
        if let Some((then, other)) = split_ternary(e) {
            let mut v = targets_at(then, depth + 1);
            v.extend(targets_at(other, depth + 1));
            return v;
        }
        let alts = split_top_op(e, &["||", "??"]);
        if alts.len() > 1 {
            return alts
                .into_iter()
                .flat_map(|a| targets_at(a, depth + 1))
                .collect();
        }
    }
    vec![link_from_expr(e)]
}

/// One JS expression as a target: a string or template literal, a
/// concatenation (`'/users/' + id`), an Angular command array
/// (`['/user', id]`), or a `{ path }` / `{ pathname }` object.
fn link_from_expr(expr: &str) -> Target {
    let e = strip_parens(expr.trim());
    if matches!(e, "null" | "undefined" | "false") {
        return Target::Skip;
    }
    let parts = split_top(e, b'+');
    if parts.len() > 1 {
        let mut s = String::new();
        for p in parts {
            match literal_text(p) {
                Some(t) => s.push_str(t),
                None => s.push_str("${...}"),
            }
        }
        return path_target(&s);
    }
    if let Some(t) = literal_text(e) {
        return path_target(t);
    }
    match e.as_bytes().first() {
        Some(b'[') => array_target(e),
        Some(b'{') => object_target(e),
        _ => Target::Dynamic,
    }
}

/// An Angular command array: the head must be a literal absolute path; later
/// literal commands are segments, `{...}` matrix / outlet parameters are
/// ignored, anything else is a `${...}` segment.
fn array_target(e: &str) -> Target {
    let Some(inner) = e.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return Target::Dynamic;
    };
    let elems: Vec<&str> = split_top(inner, b',')
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let Some(head) = elems.first().and_then(|h| literal_text(h)) else {
        return Target::Dynamic;
    };
    let mut path = head.to_string();
    for el in elems.iter().skip(1) {
        if el.starts_with('{') {
            continue;
        }
        path.push('/');
        path.push_str(literal_text(el).unwrap_or("${...}"));
    }
    path_target(&path)
}

/// `{ path: '/x' }` / `{ pathname: '/x' }`; a `{ name: 'x' }` named route (or
/// any other object) is dynamic.
fn object_target(e: &str) -> Target {
    let Some(inner) = e.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
        return Target::Dynamic;
    };
    for prop in split_top(inner, b',') {
        let Some((key, value)) = prop.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches(|c| c == '\'' || c == '"');
        if key == "path" || key == "pathname" {
            return link_from_expr(value);
        }
    }
    Target::Dynamic
}

/// The raw contents of `s` when it is exactly one string or template literal.
fn literal_text(s: &str) -> Option<&str> {
    let s = s.trim();
    let b = s.as_bytes();
    let q = *b.first()?;
    if !matches!(q, b'\'' | b'"' | b'`') {
        return None;
    }
    let end = skip_lit(b, 0, 0);
    if end != b.len() || end < 2 || b[end - 1] != q {
        return None;
    }
    s.get(1..end - 1)
}

fn strip_parens(mut e: &str) -> &str {
    while let Some(inner) = e.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        // `(a)(b)` is a call, not a wrapped expression.
        if split_top(inner, b')').len() > 1 {
            break;
        }
        e = inner.trim();
    }
    e
}

// ----------------------------------------------------------------------------
// Lexing helpers
// ----------------------------------------------------------------------------

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80
}

/// Index just past the literal opening at `i` (`'`, `"` or a backtick). A
/// `'` / `"` literal also ends at a newline (unclosed); a template literal's
/// `${...}` bodies are skipped as code, nested at most [`MAX_NEST`] deep.
fn skip_lit(b: &[u8], i: usize, nest: usize) -> usize {
    let Some(&q) = b.get(i) else {
        return b.len();
    };
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            c if c == q => return j + 1,
            b'\n' if q != b'`' => return j,
            b'$' if q == b'`' && nest < MAX_NEST && b.get(j + 1) == Some(&b'{') => {
                j = skip_block(b, j + 1, nest + 1);
            }
            _ => j += 1,
        }
    }
    b.len()
}

/// Index just past the `}` matching the `{` at `i`, literals skipped; the end
/// of `b` when unbalanced.
fn skip_block(b: &[u8], i: usize, nest: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while j < b.len() {
        match b[j] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return j + 1;
                }
            }
            b'\'' | b'"' | b'`' => {
                j = skip_lit(b, j, nest);
                continue;
            }
            _ => {}
        }
        j += 1;
    }
    b.len()
}

/// `s` split on `sep` outside brackets and literals.
fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let b = s.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut j = 0usize;
    while j < b.len() {
        match b[j] {
            b'\'' | b'"' | b'`' => {
                j = skip_lit(b, j, 0);
                continue;
            }
            c if c == sep && depth == 0 => {
                parts.push(s.get(start..j).unwrap_or(""));
                start = j + 1;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        j += 1;
    }
    parts.push(s.get(start.min(b.len())..).unwrap_or(""));
    parts
}

/// `s` split on any of the two-byte operators `ops` outside brackets and
/// literals.
fn split_top_op<'a>(s: &'a str, ops: &[&str]) -> Vec<&'a str> {
    let b = s.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut j = 0usize;
    while j < b.len() {
        match b[j] {
            b'\'' | b'"' | b'`' => {
                j = skip_lit(b, j, 0);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ if depth == 0 && ops.iter().any(|op| b[j..].starts_with(op.as_bytes())) => {
                parts.push(s.get(start..j).unwrap_or(""));
                j += 2;
                start = j;
                continue;
            }
            _ => {}
        }
        j += 1;
    }
    parts.push(s.get(start.min(b.len())..).unwrap_or(""));
    parts
}

/// `cond ? then : other` at the top level; `?.` and `??` are not a ternary.
fn split_ternary(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut depth = 0usize;
    let mut q: Option<usize> = None;
    let mut pending = 0usize;
    let mut j = 0usize;
    while j < b.len() {
        match b[j] {
            b'\'' | b'"' | b'`' => {
                j = skip_lit(b, j, 0);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'?' if depth == 0 => {
                let next = b.get(j + 1).copied();
                if next == Some(b'?') {
                    j += 2;
                    continue;
                }
                if next == Some(b'.') && !b.get(j + 2).is_some_and(u8::is_ascii_digit) {
                    j += 2;
                    continue;
                }
                if q.is_none() {
                    q = Some(j);
                } else {
                    pending += 1;
                }
            }
            b':' if depth == 0 && q.is_some() => {
                if pending == 0 {
                    let at = q.unwrap_or(0);
                    return Some((s.get(at + 1..j)?, s.get(j + 1..)?));
                }
                pending -= 1;
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// The expression starting at `from`, ending at a top-level byte in `stops`,
/// an unbalanced closer, or the end of the text. `None` past [`MAX_ARG`].
fn read_expr<'a>(text: &'a str, from: usize, stops: &[u8]) -> Option<&'a str> {
    let b = text.as_bytes();
    let limit = from.saturating_add(MAX_ARG);
    let mut depth = 0usize;
    let mut j = from;
    while j < b.len() {
        if j >= limit {
            return None;
        }
        let c = b[j];
        match c {
            b'\'' | b'"' | b'`' => {
                j = skip_lit(b, j, 0);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    return text.get(from..j).map(str::trim);
                }
                depth -= 1;
            }
            _ if depth == 0 && stops.contains(&c) => return text.get(from..j).map(str::trim),
            _ => {}
        }
        j += 1;
    }
    text.get(from.min(b.len())..).map(str::trim)
}

/// The first argument of the call whose `(` ends just before `open`.
fn first_arg(text: &str, open: usize) -> Option<&str> {
    read_expr(text, open, b",")
}

/// `src` with comments blanked to spaces (newlines kept, so byte offsets and
/// lines hold): `<!-- -->` always, `//` and `/* */` outside JS literals when
/// `js`.
fn mask_comments(src: &str, js: bool) -> Cow<'_, str> {
    let b = src.as_bytes();
    let mut out: Option<Vec<u8>> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if js && matches!(c, b'\'' | b'"' | b'`') {
            i = skip_lit(b, i, 0);
            continue;
        }
        let rest = src.get(i..).unwrap_or("");
        let end = if js && rest.starts_with("//") {
            rest.find('\n').map_or(b.len(), |k| i + k)
        } else if js && rest.starts_with("/*") {
            rest.get(2..)
                .and_then(|r| r.find("*/"))
                .map_or(b.len(), |k| i + 2 + k + 2)
        } else if rest.starts_with("<!--") {
            rest.get(4..)
                .and_then(|r| r.find("-->"))
                .map_or(b.len(), |k| i + 4 + k + 3)
        } else {
            i += 1;
            continue;
        };
        let buf = out.get_or_insert_with(|| b.to_vec());
        for byte in buf.iter_mut().take(end).skip(i) {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        i = end;
    }
    match out {
        None => Cow::Borrowed(src),
        Some(v) => String::from_utf8(v).map_or(Cow::Borrowed(src), Cow::Owned),
    }
}

/// Offsets just past `(` of every call to the bare function `name` (not a
/// member call, not its own `function name(` declaration).
fn bare_calls(text: &str, name: &str) -> Vec<usize> {
    let needle = format!("{name}(");
    let b = text.as_bytes();
    text.match_indices(needle.as_str())
        .filter(|(at, _)| *at == 0 || !(is_ident_byte(b[at - 1]) || b[at - 1] == b'.'))
        .filter(|(at, _)| {
            !text
                .get(..*at)
                .unwrap_or("")
                .trim_end()
                .ends_with("function")
        })
        .map(|(at, _)| at + needle.len())
        .collect()
}

/// Names bound to the result of `call` (`const navigate = useNavigate()`).
fn bindings_of(text: &str, call: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut names: Vec<String> = Vec::new();
    for (at, _) in text.match_indices(call) {
        if at > 0 && (is_ident_byte(b[at - 1]) || b[at - 1] == b'.') {
            continue;
        }
        let before = text.get(..at).unwrap_or("").trim_end();
        let Some(lhs) = before.strip_suffix('=') else {
            continue;
        };
        if lhs.ends_with(['=', '!', '<', '>']) {
            continue;
        }
        let lhs = lhs.trim_end();
        let n = lhs.bytes().rev().take_while(|&c| is_ident_byte(c)).count();
        if let Some(name) = lhs.get(lhs.len() - n..)
            && !name.is_empty()
            && !names.iter().any(|x| x == name)
        {
            names.push(name.to_string());
        }
    }
    names
}

// ----------------------------------------------------------------------------
// Scanners
// ----------------------------------------------------------------------------

/// Which framework forms a file can hold.
#[derive(Debug, Default)]
struct Ctx {
    /// JSX link components (`<Link>`, `<NavLink>`, `<Navigate>`).
    jsx: bool,
    /// Vue link components (`<router-link>`, `<RouterLink>`).
    vue: bool,
    /// `<Link href>` is next/link's router link.
    next_link: bool,
    /// Receivers whose `.push(` / `.replace(` navigate.
    push_recv: Vec<String>,
    /// Bare functions that navigate (`useNavigate()`'s result).
    navigate_fns: Vec<String>,
    /// `redirect(...)` / `permanentRedirect(...)` from `next/navigation`.
    next_redirect: bool,
}

impl Ctx {
    fn of(text: &str, lang: &str) -> Self {
        if lang == "angular" {
            return Self::default();
        }
        let mut ctx = Self {
            jsx: true,
            vue: true,
            next_link: text.contains("next/link"),
            next_redirect: text.contains("next/navigation"),
            ..Self::default()
        };
        let recv = |name: &str, ctx: &mut Self| {
            if !ctx.push_recv.iter().any(|r| r == name) {
                ctx.push_recv.push(name.to_string());
            }
        };
        if ["next/router", "next/navigation", "vue-router"]
            .iter()
            .any(|n| text.contains(n))
        {
            recv("router", &mut ctx);
            recv("Router", &mut ctx);
            for name in bindings_of(text, "useRouter(") {
                recv(&name, &mut ctx);
            }
        }
        if text.contains("$router") {
            recv("$router", &mut ctx);
        }
        if text.contains("react-router") {
            recv("history", &mut ctx);
            for name in bindings_of(text, "useHistory(") {
                recv(&name, &mut ctx);
            }
            ctx.navigate_fns = bindings_of(text, "useNavigate(");
        }
        ctx
    }
}

/// Router API calls: `.navigate(`, `.navigateByUrl(`, the `useNavigate()`
/// function, router `.push(` / `.replace(`, next/navigation `redirect(`.
fn scan_calls(text: &str, ctx: &Ctx, em: &mut Emitter) {
    let b = text.as_bytes();
    for needle in [".navigate(", ".navigateByUrl("] {
        for (at, _) in text.match_indices(needle) {
            if let Some(arg) = first_arg(text, at + needle.len()) {
                em.site(targets_of(arg), Tier::Router, false);
            }
        }
    }
    let mut bare: Vec<&str> = ctx.navigate_fns.iter().map(String::as_str).collect();
    if ctx.next_redirect {
        bare.extend(["redirect", "permanentRedirect"]);
    }
    for name in bare {
        for open in bare_calls(text, name) {
            if let Some(arg) = first_arg(text, open) {
                em.site(targets_of(arg), Tier::Router, false);
            }
        }
    }
    for recv in &ctx.push_recv {
        for method in [".push(", ".replace("] {
            let needle = format!("{recv}{method}");
            for (at, _) in text.match_indices(needle.as_str()) {
                if at > 0 && is_ident_byte(b[at - 1]) {
                    continue;
                }
                if let Some(arg) = first_arg(text, at + needle.len()) {
                    em.site(targets_of(arg), Tier::Router, false);
                }
            }
        }
    }
}

/// Plain-tier navigation: `location.href = ...`, `location.assign(...)`,
/// `location.replace(...)`.
fn scan_location(text: &str, em: &mut Emitter) {
    let b = text.as_bytes();
    let bounded = |at: usize| at == 0 || !is_ident_byte(b[at - 1]);
    for (at, needle) in text.match_indices("location.href") {
        if !bounded(at) {
            continue;
        }
        let after = at + needle.len();
        let rest = text.get(after..).unwrap_or("");
        let value = rest.trim_start();
        if !value.starts_with('=') || value.starts_with("==") {
            continue;
        }
        let from = after + (rest.len() - value.len()) + 1;
        if let Some(expr) = read_expr(text, from, b";,\n") {
            em.site(targets_of(expr), Tier::Href, false);
        }
    }
    for needle in ["location.assign(", "location.replace("] {
        for (at, _) in text.match_indices(needle) {
            if !bounded(at) {
                continue;
            }
            if let Some(arg) = first_arg(text, at + needle.len()) {
                em.site(targets_of(arg), Tier::Href, false);
            }
        }
    }
}

/// Origin share links (see the module doc).
fn scan_origin(text: &str, em: &mut Emitter) {
    let b = text.as_bytes();
    for (at, needle) in text.match_indices("location.origin") {
        let end = at + needle.len();
        if b.get(end).is_some_and(|&c| is_ident_byte(c)) {
            continue;
        }
        // The whole receiver chain: `window.location.origin`.
        let mut start = at;
        while start > 0 && (is_ident_byte(b[start - 1]) || b[start - 1] == b'.') {
            start -= 1;
        }
        let rest = text.get(end..).unwrap_or("");
        let trimmed = rest.trim_start();
        let after_ws = end + (rest.len() - trimmed.len());
        if start >= 3 && b[start - 1] == b'{' && b[start - 2] == b'$' && b[start - 3] == b'`' {
            // `${window.location.origin}/path...` at the start of a template.
            if !trimmed.starts_with('}') {
                continue;
            }
            let tick = start - 3;
            if in_http_call(text, tick) {
                continue;
            }
            let window = clip(text.get(tick..).unwrap_or(""), MAX_ARG);
            let close = tick + skip_lit(window.as_bytes(), 0, 0);
            let body_end = if b.get(close.wrapping_sub(1)) == Some(&b'`') && close > after_ws + 1 {
                close - 1
            } else {
                continue;
            };
            origin_site(text.get(after_ws + 1..body_end).unwrap_or(""), em);
        } else if trimmed.starts_with('+') {
            // `window.location.origin + '/path' + id`.
            if in_http_call(text, start) {
                continue;
            }
            let Some(expr) = read_expr(text, start, b";,\n") else {
                continue;
            };
            let parts = split_top(expr, b'+');
            let mut s = String::new();
            for p in parts.iter().skip(1) {
                s.push_str(literal_text(p).unwrap_or("${...}"));
            }
            origin_site(&s, em);
        }
    }
}

/// The text after the origin: a path is a router-tier link; a `${...}` head
/// (`${origin}${base}/x`) is dynamic; anything else is not a page link.
fn origin_site(after: &str, em: &mut Emitter) {
    if after.starts_with('/') && !after.starts_with("//") {
        em.site(vec![path_target(after)], Tier::Router, true);
    } else if after.starts_with("${") {
        em.site(vec![Target::Dynamic], Tier::Router, true);
    }
}

/// The expression at `at` is the first argument of an HTTP-client call.
fn in_http_call(text: &str, at: usize) -> bool {
    let before = text.get(..at).unwrap_or("").trim_end();
    let Some(callee) = before.strip_suffix('(') else {
        return false;
    };
    let callee = callee.trim_end();
    let n = callee
        .bytes()
        .rev()
        .take_while(|&c| is_ident_byte(c))
        .count();
    callee
        .get(callee.len() - n..)
        .is_some_and(|name| HTTP_CALLEES.contains(&name))
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

// ----------------------------------------------------------------------------
// Tags
// ----------------------------------------------------------------------------

/// One attribute value as written.
#[derive(Debug, Clone, Copy)]
enum AttrVal<'a> {
    /// `"x"` / `'x'`, delimiters dropped.
    Quoted(&'a str),
    /// JSX `{x}`, braces dropped.
    Braced(&'a str),
    /// Unquoted `x`.
    Bare(&'a str),
    /// A boolean attribute (`download`).
    Flag,
}

/// One opening tag (HTML, Angular or Vue template, JSX).
#[derive(Debug)]
struct Tag<'a> {
    name: &'a str,
    attrs: Vec<(&'a str, AttrVal<'a>)>,
    end: usize,
}

impl<'a> Tag<'a> {
    fn attr(&self, name: &str) -> Option<AttrVal<'a>> {
        self.attrs.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    }
}

/// Attribute-name bytes, Angular (`[routerLink]`, `(click)`, `*ngFor`,
/// `#ref`) and Vue (`:to`, `v-bind:to`, `@click`) syntax included.
fn is_attr_byte(c: u8) -> bool {
    is_ident_byte(c)
        || matches!(
            c,
            b'-' | b':' | b'.' | b'[' | b']' | b'(' | b')' | b'*' | b'#' | b'@'
        )
}

/// Parse the opening tag whose `<` is at `lt`. `None` when it is not one
/// within [`MAX_TAG`] bytes (a comparison, a generic, a closing tag).
fn read_tag(s: &str, lt: usize) -> Option<Tag<'_>> {
    let b = s.as_bytes();
    let mut limit = lt.saturating_add(MAX_TAG).min(b.len());
    while limit > lt && !s.is_char_boundary(limit) {
        limit -= 1;
    }
    let mut j = lt + 1;
    if !b.get(j).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    while j < limit && (is_ident_byte(b[j]) || matches!(b[j], b'-' | b'.' | b':')) {
        j += 1;
    }
    let name = s.get(lt + 1..j)?;
    let mut attrs = Vec::new();
    loop {
        while j < limit && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= limit {
            return None;
        }
        match b[j] {
            b'>' => {
                return Some(Tag {
                    name,
                    attrs,
                    end: j + 1,
                });
            }
            b'/' if b.get(j + 1) == Some(&b'>') => {
                return Some(Tag {
                    name,
                    attrs,
                    end: j + 2,
                });
            }
            b'{' => {
                // `{...spread}`.
                j = skip_block(b.get(..limit)?, j, 0);
                continue;
            }
            _ => {}
        }
        let at = j;
        while j < limit && is_attr_byte(b[j]) {
            j += 1;
        }
        if j == at {
            return None;
        }
        let attr = s.get(at..j)?;
        while j < limit && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if b.get(j) != Some(&b'=') {
            attrs.push((attr, AttrVal::Flag));
            continue;
        }
        j += 1;
        while j < limit && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let v = match b.get(j) {
            Some(&q @ (b'"' | b'\'')) => {
                let close = j + 1 + s.get(j + 1..limit)?.find(q as char)?;
                let v = AttrVal::Quoted(s.get(j + 1..close)?);
                j = close + 1;
                v
            }
            Some(b'{') => {
                let close = skip_block(b.get(..limit)?, j, 0);
                if close > limit || b.get(close.wrapping_sub(1)) != Some(&b'}') {
                    return None;
                }
                let v = AttrVal::Braced(s.get(j + 1..close - 1)?);
                j = close;
                v
            }
            Some(_) => {
                let from = j;
                while j < limit && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                    j += 1;
                }
                AttrVal::Bare(s.get(from..j)?)
            }
            None => return None,
        };
        attrs.push((attr, v));
    }
}

/// An attribute's targets. `bound`: the value is a JS expression (`:to`,
/// `[routerLink]`) even when quoted; a JSX `{...}` value always is.
fn attr_targets(v: AttrVal<'_>, bound: bool) -> Vec<Target> {
    match v {
        AttrVal::Quoted(s) | AttrVal::Bare(s) if bound => targets_of(s),
        AttrVal::Quoted(s) | AttrVal::Bare(s) => vec![path_target(s)],
        AttrVal::Braced(s) => targets_of(s),
        AttrVal::Flag => Vec::new(),
    }
}

/// `<Navigate>` at `lt` is the value of a route `element` (`element={<Navigate
/// />}`, `element: <Navigate />`), looking through `<Suspense>` / `<>` /
/// `<Fragment>` wrappers: LA.6b emits that one as the route's redirect.
fn inside_route_element(text: &str, lt: usize) -> bool {
    let mut before = text.get(..lt).unwrap_or("");
    for _ in 0..4 {
        let t = before.trim_end_matches(|c: char| c.is_whitespace() || c == '(');
        if t.ends_with("element={") || t.ends_with("element=") || t.ends_with("element:") {
            return true;
        }
        let Some(open_tag) = t.strip_suffix('>') else {
            return false;
        };
        let Some(open) = open_tag.rfind('<') else {
            return false;
        };
        let name = open_tag
            .get(open + 1..)
            .unwrap_or("")
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if !matches!(
            name.strip_prefix("React.").unwrap_or(name),
            "" | "Suspense" | "Fragment" | "StrictMode"
        ) {
            return false;
        }
        before = open_tag.get(..open).unwrap_or("");
    }
    false
}

/// Every link attribute on every tag (see the module doc for the forms).
fn scan_tags(text: &str, ctx: &Ctx, em: &mut Emitter) {
    let mut resume = 0usize;
    for (lt, _) in text.match_indices('<') {
        if lt < resume {
            continue;
        }
        let Some(tag) = read_tag(text, lt) else {
            continue;
        };
        resume = tag.end;
        let short = tag.name.rsplit('.').next().unwrap_or(tag.name);
        let mut next_link = false;
        if ctx.jsx && matches!(short, "Link" | "NavLink" | "Navigate") {
            if short == "Navigate" && inside_route_element(text, lt) {
                continue;
            }
            if let Some(v) = tag.attr("to") {
                em.site(attr_targets(v, false), Tier::Router, false);
            } else if short == "Link"
                && ctx.next_link
                && let Some(v) = tag.attr("href")
            {
                em.site(attr_targets(v, false), Tier::Router, false);
                next_link = true;
            }
        }
        if ctx.vue && matches!(tag.name, "router-link" | "RouterLink") {
            if let Some(v) = tag.attr("to") {
                em.site(attr_targets(v, false), Tier::Router, false);
            } else if let Some(v) = tag.attr(":to").or_else(|| tag.attr("v-bind:to")) {
                em.site(attr_targets(v, true), Tier::Router, false);
            }
        }
        if let Some(v) = tag.attr("routerLink") {
            em.site(attr_targets(v, false), Tier::Router, false);
        }
        if let Some(v) = tag.attr("[routerLink]") {
            em.site(attr_targets(v, true), Tier::Router, false);
        }
        if next_link || matches!(tag.name, "base" | "link") {
            continue;
        }
        if let Some(v) = tag.attr("href") {
            em.site(attr_targets(v, false), Tier::Href, false);
        } else if let Some(v) = tag
            .attr(":href")
            .or_else(|| tag.attr("v-bind:href"))
            .or_else(|| tag.attr("[href]"))
        {
            em.site(attr_targets(v, true), Tier::Href, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{GRAPH_TYPE, node_kind};
    use repo_graph_core::RepoId;

    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, RepoId(1), node_kind::MODULE, "src::app::x")
    }

    fn links(out: &NavLinkOut) -> Vec<&str> {
        out.refs
            .iter()
            .map(|r| {
                assert_eq!(r.category, edge_category::NAVIGATES_TO);
                assert_eq!(r.from, module_id());
                assert_eq!(r.from_module, module_id());
                match &r.qualifier {
                    CallQualifier::Bare(s) => s.as_str(),
                    q => panic!("not Bare: {q:?}"),
                }
            })
            .collect()
    }

    fn ts(src: &str) -> NavLinkOut {
        extract_nav_links(src, "typescript", module_id())
    }

    #[test]
    fn nav_links_angular_router_api() {
        let src = r#"
import { Router } from '@angular/router';
export class LoginComponent {
  constructor(private router: Router) {}
  a() { this.router.navigate(['/home']); }
  b(id: string) { void this.router.navigate(['/user', id]); }
  c() { this.router.navigateByUrl('/verify-email'); }
  d(p: string) { this.router.navigate([p]); }
  e() { this.router.navigate(['child'], { relativeTo: this.route }); }
  f() { this.router.navigate([], { queryParams: { a: 1 } }); }
  g() { this.router.navigate(['/groups'], { queryParams: { 'chat-name': 1 } }); }
  h(redirect: string) { this.router.navigateByUrl(redirect); }
  i() { this.router.navigateByUrl(`/orders/${this.id}/edit?tab=2`); }
  j() { this.router.navigate(['/a/', 'b', 7]); }
}
"#;
        let out = extract_nav_links(src, "angular", module_id());
        assert_eq!(
            links(&out),
            [
                "/home",
                "/user/${...}",
                "/groups",
                "/a/b/${...}",
                "/verify-email",
                "/orders/${...}/edit",
            ]
        );
        assert_eq!(
            (out.router, out.href, out.origin, out.template),
            (6, 0, 0, 0)
        );
        // `[p]`, `['child']`, `[]`, `navigateByUrl(redirect)`.
        assert_eq!(out.dynamic_skipped, 4);
    }

    #[test]
    fn nav_links_angular_template() {
        let html = r##"
<base href="/">
<link rel="icon" href="/favicon.ico">
<a class="nav-brand" routerLink="/home">Home</a>
<div *ngFor="let m of members">
  <a [routerLink]="m.id ? ['/user', m.id] : null">user</a>
  <a [routerLink]="'/groups'" (click)="router.navigate(['/activities'])">g</a>
  <a routerLink="/user/{{ m.id }}">u</a>
</div>
<!-- <a routerLink="/old">old</a> -->
<a routerLink="child">relative</a>
<img src="/logo.png">
<a href="/favicon.ico">i</a>
<a href="/quokka_nobg.png" download>png</a>
<a href="mailto:press@quokk4.net">mail</a>
<a href="https://quokk4.net">site</a>
<a href="#top">top</a>
<a href="/logout">out</a>
<a [href]="docsUrl">docs</a>
<a routerLink="/home">again</a>
"##;
        let out = extract_template_links(html, module_id());
        assert_eq!(
            links(&out),
            [
                "/activities",
                "/home",
                "/user/${...}",
                "/groups",
                "href:/logout"
            ]
        );
        assert_eq!(out.router, 4);
        assert_eq!(out.href, 1);
        assert_eq!(out.template, 5);
        // `routerLink="child"` and `[href]="docsUrl"`.
        assert_eq!(out.dynamic_skipped, 2);
    }

    #[test]
    fn nav_links_angular_inline_template() {
        let src = "import { Component } from '@angular/core';\n\
                   @Component({ selector: 'x', template: `<a routerLink=\"/about\">a</a><a href=\"/terms\">t</a>` })\n\
                   export class X {}\n";
        let out = extract_nav_links(src, "angular", module_id());
        assert_eq!(links(&out), ["/about", "href:/terms"]);
    }

    #[test]
    fn nav_links_react_router() {
        let src = r#"
import { createBrowserRouter, Link, NavLink, Navigate, Route, useNavigate } from 'react-router-dom';
export const router = createBrowserRouter([
  { path: '/dashboard', element: <Dashboard /> },
  { path: '/old', element: <Navigate to="/dashboard" replace /> },
]);
const tree = <Route path="/legacy" element={<Suspense><Navigate to="/settings" /></Suspense>} />;
export function Nav({ id }: { id: number }) {
  const go = useNavigate();
  if (!id) return <Navigate to="/login" />;
  return (
    <div>
      <Link to="/dashboard">D</Link>
      <NavLink to={`/users/${id}`}>U</NavLink>
      <button onClick={() => go("/settings")}>S</button>
      <button onClick={() => go(-1)}>Back</button>
      <Link to={{ pathname: '/billing', search: '?a=1' }}>B</Link>
      <a href="/auth/google">G</a>
      <a href={`/files/${id}.pdf`}>F</a>
    </div>
  );
}
"#;
        let out = ts(src);
        assert_eq!(
            links(&out),
            [
                "/settings",
                "/login",
                "/dashboard",
                "/users/${...}",
                "/billing",
                "href:/auth/google",
            ]
        );
        // The two `<Navigate>` route elements are LA.6b's redirects: `/dashboard`
        // comes from the `<Link>`, and `/settings` from `go(...)`, not them.
        assert_eq!((out.router, out.href), (5, 1), "`${{id}}.pdf` is an asset");
        assert_eq!(out.dynamic_skipped, 1, "go(-1)");
    }

    #[test]
    fn nav_links_navigate_in_a_route_element_is_la6b_s() {
        let src = r#"
import { Navigate } from 'react-router-dom';
const routes = [
  { path: '/a', element: <Suspense fallback="x"><Navigate to="/b" /></Suspense> },
  { path: '/c', element: (
      <Navigate to="/d" />
  ) },
];
function Guard() { return <><Navigate to="/e" /></>; }
"#;
        assert!(inside_route_element(
            src,
            src.find("<Navigate to=\"/b\"").unwrap_or(0)
        ));
        assert_eq!(links(&ts(src)), ["/e"]);
    }

    #[test]
    fn nav_links_next() {
        let src = r#"
import Link from "next/link";
import { useRouter } from "next/router";
import { redirect } from "next/navigation";
export default function Home() {
  const r = useRouter();
  if (!user) redirect('/login');
  return (
    <main>
      <Link href="/users/7">User</Link>
      <button onClick={() => r.push("/orders/9")}>Order</button>
      <button onClick={() => router.replace({ pathname: '/pricing' })}>P</button>
    </main>
  );
}
"#;
        let out = ts(src);
        assert_eq!(links(&out), ["/login", "/pricing", "/orders/9", "/users/7"]);
        assert_eq!(
            (out.router, out.href),
            (4, 0),
            "a next/link href is router tier"
        );

        // Without next/link, `<Link href>` is a plain anchor.
        let out = ts("import { useNavigate } from 'react-router';\n<Link href=\"/x\">x</Link>\n");
        assert_eq!(links(&out), ["href:/x"]);
    }

    #[test]
    fn nav_links_vue() {
        let src = r#"
<template>
  <router-link to="/cart">Cart</router-link>
  <RouterLink :to="`/users/${u.id}`">U</RouterLink>
  <router-link :to="{ name: 'home' }">H</router-link>
  <RouterLink v-bind:to="{ path: '/help' }">?</RouterLink>
</template>
<script setup lang="ts">
import { useRouter } from 'vue-router';
const router = useRouter();
function go() {
  router.push('/cart');
  router.push({ path: '/orders' });
  router.push({ name: 'cart' });
}
export default { methods: { back() { this.$router.push('/account'); } } };
</script>
"#;
        let out = extract_nav_links(src, "vue", module_id());
        assert_eq!(
            links(&out),
            ["/cart", "/orders", "/account", "/users/${...}", "/help"]
        );
        assert_eq!(out.router, 5);
        // `{ name: 'cart' }` push and the `{ name: 'home' }` :to.
        assert_eq!(out.dynamic_skipped, 2);
    }

    #[test]
    fn nav_links_origin_share_links() {
        let src = r#"
export class ProfileComponent {
  get shareLink(): string {
    return `${window.location.origin}/connect?ref=${this.publicId}`;
  }
  other() { return window.location.origin + '/user/' + this.id + '/card'; }
  base() { return `${location.origin}`; }
  api() { return fetch(`${window.location.origin}/api/users`); }
  api2() { return this.http.get(window.location.origin + '/api/orders'); }
  prefixed() { return `${location.origin}${this.base}/x`; }
  host = typeof window !== 'undefined' ? window.location.origin : 'http://localhost';
}
"#;
        let out = ts(src);
        assert_eq!(links(&out), ["/connect", "/user/${...}/card"]);
        assert_eq!((out.router, out.origin), (2, 2));
        assert_eq!(out.dynamic_skipped, 1, "`${{origin}}${{base}}/x`");
    }

    #[test]
    fn nav_links_plain_location() {
        let src = r#"
import { Router } from '@angular/router';
export class A {
  a() { window.location.href = '/logout'; }
  b() { location.assign('/signin?next=1'); }
  c() { if (window.location.href === '/x') {} }
  d(p: string) { window.location.href = this.providerUrl(p); }
  e() { document.location.replace("https://other.host/x"); }
}
"#;
        let out = ts(src);
        assert_eq!(links(&out), ["href:/logout", "href:/signin"]);
        assert_eq!(out.dynamic_skipped, 1);
    }

    #[test]
    fn nav_links_gate_keeps_backend_files_out() {
        let src = r#"
import express from 'express';
const app = express();
app.get('/old', (req, res) => res.redirect('/x'));
const html = '<a href="/docs">docs</a>';
function redirect(url: string) { return url; }
redirect('/y');
"#;
        let out = ts(src);
        assert!(out.refs.is_empty(), "{:?}", links(&out));
        assert_eq!(out.dynamic_skipped, 0);
    }

    #[test]
    fn nav_links_comments_are_not_links() {
        let src = "import { Router } from '@angular/router';\n\
                   // this.router.navigate(['/old']);\n\
                   /* this.router.navigateByUrl('/older'); */\n\
                   const s = 'http://not-a-comment'; this.router.navigate(['/live']);\n";
        assert_eq!(links(&ts(src)), ["/live"]);
    }

    #[test]
    fn nav_links_multibyte_and_truncated_input_never_panics() {
        for src in [
            "import '@angular/router'; this.router.navigate(['/ü",
            "import 'react-router'; <Link to=\"é",
            "import 'react-router'; <Link to={`/ü/${",
            "`${window.location.origin}",
            "`${window.location.origin}/日本/${x}`",
            "window.location.origin + '/ä",
            "import 'vue-router'; router.push({ path: '/ö",
            "import 'vue-router'; <router-link :to=\"`/ö/${a ? 'x' : \"y\"}`\">",
            "routerLink <a routerLink='/é'> [routerLink]=\"['/ß', é ? 'x' : 'y']\"",
            "import '@angular/router'; x ? y : z ?. ?? ? : <a",
            "import '@angular/router'; `${`${`${`${`",
        ] {
            let _ = extract_nav_links(src, "typescript", module_id());
            let _ = extract_nav_links(src, "angular", module_id());
            let _ = extract_template_links(src, module_id());
        }
        let out = extract_nav_links(
            "`${window.location.origin}/日本/${x}`",
            "typescript",
            module_id(),
        );
        assert_eq!(links(&out), ["/日本/${...}"]);
    }

    #[test]
    fn nav_links_asset_paths() {
        for p in [
            "/favicon.ico",
            "/quokka_nobg.png",
            "/manifest.webmanifest",
            "/a/b.pdf",
            "/x.css",
        ] {
            assert!(is_asset_path(p), "{p}");
        }
        for p in [
            "/",
            "/home",
            "/v1.2",
            "/.well-known",
            "/u/${...}",
            "/users/:id",
        ] {
            assert!(!is_asset_path(p), "{p}");
        }
    }

    #[test]
    fn nav_links_expression_forms() {
        assert_eq!(link_from_expr("'/a?x=1#f'"), Target::Link("/a".into()));
        assert_eq!(
            link_from_expr("'/users/' + id"),
            Target::Link("/users/${...}".into())
        );
        assert_eq!(
            link_from_expr("`/p-${id}/x`"),
            Target::Link("/${...}/x".into())
        );
        assert_eq!(
            link_from_expr("('/wrapped')"),
            Target::Link("/wrapped".into())
        );
        assert_eq!(link_from_expr("'//cdn.host/x'"), Target::Skip);
        assert_eq!(link_from_expr("'tel:123'"), Target::Skip);
        assert_eq!(link_from_expr("null"), Target::Skip);
        assert_eq!(link_from_expr("base + '/x'"), Target::Dynamic);
        assert_eq!(link_from_expr("'rel/x'"), Target::Dynamic);
        assert_eq!(
            targets_of("a?.b ? '/x' : c ?? '/y'"),
            [
                Target::Link("/x".into()),
                Target::Dynamic,
                Target::Link("/y".into())
            ]
        );
    }
}
