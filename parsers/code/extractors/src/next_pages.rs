//! Next.js file-system page routes (LA.6d): the `pages/` and `app/` routers,
//! where the file tree IS the route table.
//!
//! # Why this is not a per-file extractor
//!
//! A `src/pages/Home.tsx` folder is idiomatic in Vite / CRA apps that route
//! with React Router; minting `/Home` there would fabricate a page and hide
//! real dead links. What decides it is repo-level (a `package.json` declaring
//! `next` at or above the file), and baking that into a per-file parse would
//! poison the parse cache, which is keyed by the file's own content. So the
//! engine calls this module from a POST-CACHE graft
//! (`engine/src/build/grafts.rs`), over every walked file, cached or not:
//! [`NextRoots`] finds the Next project roots, [`graft_page`] mints one page.
//!
//! # Path rules ([`page_route`], relative to the Next root, `/`-separated)
//!
//! | router | file | route |
//! |--------|------|-------|
//! | pages | `pages/index.tsx`, `pages/users/index.tsx` | `/`, `/users` |
//! | pages | `pages/users/[id].tsx` | `/users/:id` |
//! | pages | `pages/api/**`, `_app`, `_document`, `_error`, `_middleware` | none |
//! | app | `app/page.tsx`, `app/orders/[id]/page.tsx` | `/`, `/orders/:id` |
//! | app | `app/(group)/x/page.tsx`, `app/@slot/x/page.tsx` | `/x` (group / slot dropped) |
//! | app | `(.)x` / `(..)x` / `(...)x` intercepting segment, `_private` folder | none |
//! | both | `[...slug]` / `[[...slug]]` as the LAST segment | kept verbatim, `catchall` |
//!
//! `src/pages` / `src/app` are the same routers. Only `page.*` is an app-router
//! page (`route.*` is an API handler, which ts_routes.rs owns), and `pages/api`
//! is excluded, so a page is never also a server route.
//!
//! A catch-all keeps Next's bracket spelling: `graph/src/nav.rs` classifies
//! `[...x]` as a REQUIRED scoped catch-all (one or more segments) and
//! `[[...x]]` as an OPTIONAL one (it also serves the bare prefix), exactly
//! Next's semantics. A `:x*` spelling would read as optional for both. A
//! `[id]` segment becomes `:id`, the same spelling ts_routes.rs gives Next API
//! routes.
//!
//! # Emission
//!
//! Through LA.6b's [`emit_nav_routes`]: one nav ROUTE `page:<path>` (ROUTE_METHOD
//! `GET` + the nav ORIGIN, so the HTTP resolver never pairs a client call with
//! it) and a HANDLED_BY ref to the file's default-exported component
//! ([`default_export_component`]), bound by the graph builder through the
//! file's module symbols. A page with no nameable default export keeps its
//! ROUTE without a HANDLED_BY.

use std::borrow::Cow;

use repo_graph_core::{NodeId, RepoId};

use crate::nav_routes::{NavRouteOut, RouteRecord, emit_nav_routes};

/// Which Next router a page file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageRouter {
    /// `pages/` (or `src/pages/`).
    Pages,
    /// `app/` (or `src/app/`).
    App,
}

/// The route one Next page file serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRoute {
    /// Leading `/`, `[id]` written `:id`, a catch-all kept as `[...x]` /
    /// `[[...x]]`. The root page is `/`.
    pub path: String,
    /// The last segment is `[...x]` or `[[...x]]`.
    pub catchall: bool,
    pub router: PageRouter,
}

/// Does this `package.json` declare `next` in `dependencies`,
/// `devDependencies` or `peerDependencies`? Malformed JSON is `false`.
pub fn declares_next(package_json: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(package_json) else {
        return false;
    };
    ["dependencies", "devDependencies", "peerDependencies"]
        .iter()
        .any(|k| {
            v.get(k)
                .and_then(serde_json::Value::as_object)
                .is_some_and(|deps| deps.contains_key("next"))
        })
}

/// The project roots of one repo, read off its `package.json` files, so a page
/// file can be judged by the root that OWNS it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NextRoots {
    /// `(dir, declares next)`, sorted by dir. `""` is the repo root.
    roots: Vec<(String, bool)>,
}

impl NextRoots {
    /// Build from `(repo-relative path, source)` pairs; only `package.json`
    /// files count, and never one under `node_modules/` (a vendored package is
    /// not a project).
    pub fn from_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut roots: Vec<(String, bool)> = files
            .into_iter()
            .filter_map(|(path, source)| {
                let dir = match path.rsplit_once('/') {
                    Some((dir, "package.json")) => dir,
                    None if path == "package.json" => "",
                    _ => return None,
                };
                if dir == "node_modules"
                    || dir.starts_with("node_modules/")
                    || dir.contains("/node_modules/")
                    || dir.ends_with("/node_modules")
                {
                    return None;
                }
                Some((dir.to_string(), declares_next(source)))
            })
            .collect();
        roots.sort();
        roots.dedup_by(|a, b| a.0 == b.0);
        Self { roots }
    }

    /// Roots whose `package.json` declares `next`.
    pub fn next_count(&self) -> usize {
        self.roots.iter().filter(|(_, next)| *next).count()
    }

    /// `path` relative to the root that owns it, when that root is a Next
    /// root. The owner is the LONGEST enclosing root (segment-bounded), so a
    /// Vite app nested in a Next monorepo keeps its `src/pages/` out.
    pub fn rel_under_next_root<'p>(&self, path: &'p str) -> Option<&'p str> {
        let (dir, next) = self
            .roots
            .iter()
            .filter(|(dir, _)| {
                dir.is_empty()
                    || path
                        .strip_prefix(dir.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|(dir, _)| dir.len())?;
        if !*next {
            return None;
        }
        if dir.is_empty() {
            Some(path)
        } else {
            path.get(dir.len() + 1..)
        }
    }
}

/// The route a file serves, given its path relative to the Next root (see the
/// module doc's table). `None` for anything that is not a page.
pub fn page_route(rel_under_root: &str) -> Option<PageRoute> {
    let body = rel_under_root.strip_prefix("src/").unwrap_or(rel_under_root);
    if let Some(rest) = body.strip_prefix("pages/") {
        return pages_router(rest);
    }
    if let Some(rest) = body.strip_prefix("app/") {
        return app_router(rest);
    }
    None
}

/// The file name without its JS-family extension; `None` for any other file
/// and for declaration / test / story files, which are never pages.
fn page_stem(file: &str) -> Option<&str> {
    if file.ends_with(".d.ts") {
        return None;
    }
    let stem = [".tsx", ".jsx", ".ts", ".js"]
        .iter()
        .find_map(|ext| file.strip_suffix(ext))?;
    if [".test", ".spec", ".stories"]
        .iter()
        .any(|infix| stem.ends_with(infix))
    {
        return None;
    }
    Some(stem)
}

fn pages_router(rest: &str) -> Option<PageRoute> {
    let (dirs, file) = match rest.rsplit_once('/') {
        Some((dirs, file)) => (dirs, file),
        None => ("", rest),
    };
    let stem = page_stem(file)?;
    if dirs.split('/').next() == Some("api") {
        return None;
    }
    if dirs.is_empty() && matches!(stem, "_app" | "_document" | "_error" | "api") {
        return None;
    }
    if stem == "_middleware" {
        return None;
    }
    let mut segs: Vec<&str> = dirs.split('/').filter(|s| !s.is_empty()).collect();
    if stem != "index" {
        segs.push(stem);
    }
    build_route(&segs, PageRouter::Pages)
}

fn app_router(rest: &str) -> Option<PageRoute> {
    let (dirs, file) = match rest.rsplit_once('/') {
        Some((dirs, file)) => (dirs, file),
        None => ("", rest),
    };
    if page_stem(file)? != "page" {
        return None;
    }
    let mut segs: Vec<&str> = Vec::new();
    for seg in dirs.split('/').filter(|s| !s.is_empty()) {
        if seg.starts_with("(.") || seg.starts_with('_') {
            // An intercepting route shadows another route; a private folder
            // opts its whole subtree out of routing.
            return None;
        }
        if (seg.starts_with('(') && seg.ends_with(')')) || seg.starts_with('@') {
            continue;
        }
        segs.push(seg);
    }
    build_route(&segs, PageRouter::App)
}

/// Compose URL segments into a [`PageRoute`]: `[id]` -> `:id`, a catch-all
/// kept verbatim and only as the last segment. `None` for a segment that is
/// no URL (whitespace, a quote, an empty or malformed bracket).
fn build_route(segs: &[&str], router: PageRouter) -> Option<PageRoute> {
    let mut out: Vec<Cow<'_, str>> = Vec::with_capacity(segs.len());
    let mut catchall = false;
    for (i, seg) in segs.iter().enumerate() {
        if seg.is_empty()
            || seg.chars().any(|c| {
                c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '`' | '?' | '#')
            })
        {
            return None;
        }
        match bracket(seg)? {
            Bracket::Literal => out.push(Cow::Borrowed(seg)),
            Bracket::Param(name) => out.push(Cow::Owned(format!(":{name}"))),
            Bracket::CatchAll => {
                if i + 1 != segs.len() {
                    return None;
                }
                catchall = true;
                out.push(Cow::Borrowed(seg));
            }
        }
    }
    Some(PageRoute {
        path: format!("/{}", out.join("/")),
        catchall,
        router,
    })
}

enum Bracket<'a> {
    Literal,
    Param(&'a str),
    CatchAll,
}

/// Classify one segment; `None` when it opens a bracket it does not close
/// cleanly or names nothing.
fn bracket(seg: &str) -> Option<Bracket<'_>> {
    if !seg.contains('[') && !seg.contains(']') {
        return Some(Bracket::Literal);
    }
    let named = |inner: &str| {
        !inner.is_empty()
            && inner
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '$')
    };
    if let Some(inner) = seg.strip_prefix("[[...").and_then(|s| s.strip_suffix("]]")) {
        return named(inner).then_some(Bracket::CatchAll);
    }
    if let Some(inner) = seg.strip_prefix("[...").and_then(|s| s.strip_suffix(']')) {
        return named(inner).then_some(Bracket::CatchAll);
    }
    let inner = seg.strip_prefix('[').and_then(|s| s.strip_suffix(']'))?;
    named(inner).then_some(Bracket::Param(inner))
}

/// Byte cap on the default-export expression read after `export default`.
const MAX_EXPR: usize = 512;

/// The name of the component a page file default-exports:
/// `export default [async] function X`, `export default class X`,
/// `export { X as default }`, or `export default <expr>` whose only
/// capitalised identifier declared in the file is `X` (`export default X`,
/// `const X = ...; export default X`, `export default withAuth(X)`). An
/// anonymous function, an arrow or an object literal names nothing: `None`.
/// Comments are masked first, so a commented-out export never counts.
pub fn default_export_component(source: &str) -> Option<String> {
    let text = mask_comments(source);
    let text = text.as_ref();
    let mut from = 0;
    while let Some(rel) = text.get(from..).and_then(|s| s.find("export")) {
        let at = from + rel;
        from = at + "export".len();
        if !word_bounded(text, at, "export".len()) {
            continue;
        }
        let after = text.get(from..).unwrap_or("").trim_start();
        if let Some(rest) = after.strip_prefix("default")
            && rest.starts_with(|c: char| c.is_whitespace())
        {
            return default_target(text, rest.trim_start());
        }
        if let Some(rest) = after.strip_prefix('{')
            && let Some(name) = export_list_default(rest)
        {
            return declared_in(text, name).then(|| name.to_string());
        }
    }
    None
}

/// What `export default <rest>` names.
fn default_target(text: &str, rest: &str) -> Option<String> {
    let decl = rest
        .strip_prefix("async")
        .filter(|r| r.starts_with(|c: char| c.is_whitespace()))
        .map_or(rest, str::trim_start);
    if let Some(r) = decl.strip_prefix("function") {
        let r = r.trim_start_matches(|c: char| c.is_whitespace() || c == '*');
        return ident_at(r).map(str::to_string);
    }
    if let Some(r) = decl.strip_prefix("class")
        && r.starts_with(|c: char| c.is_whitespace())
    {
        return ident_at(r.trim_start()).map(str::to_string);
    }
    let expr = expression(rest);
    if expr.is_empty() || expr.contains("=>") || expr.starts_with(['{', '(', '[', '<']) {
        return None;
    }
    let mut found: Option<&str> = None;
    for name in identifiers(expr) {
        if !name.starts_with(|c: char| c.is_ascii_uppercase()) || !declared_in(text, name) {
            continue;
        }
        match found {
            None => found = Some(name),
            Some(f) if f == name => {}
            Some(_) => return None,
        }
    }
    found.map(str::to_string)
}

/// `X as default` inside an `export { ... }` list.
fn export_list_default(rest: &str) -> Option<&str> {
    let body = rest.get(..rest.find('}')?)?;
    body.split(',').find_map(|item| {
        let mut words = item.split_whitespace();
        match (words.next(), words.next(), words.next(), words.next()) {
            (Some(name), Some("as"), Some("default"), None) => ident_at(name).filter(|i| *i == name),
            _ => None,
        }
    })
}

/// The export expression: up to the first `;`, or the first newline outside
/// brackets, clipped to [`MAX_EXPR`].
fn expression(rest: &str) -> &str {
    let mut depth = 0i32;
    let mut end = rest.len();
    for (i, c) in rest.char_indices() {
        if i >= MAX_EXPR {
            end = i;
            break;
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ';' => {
                end = i;
                break;
            }
            '\n' if depth <= 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    rest.get(..end).unwrap_or("").trim()
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The identifier `s` starts with.
fn ident_at(s: &str) -> Option<&str> {
    let end = s.find(|c: char| !is_ident_char(c)).unwrap_or(s.len());
    let id = s.get(..end)?;
    (!id.is_empty() && !id.starts_with(|c: char| c.is_ascii_digit())).then_some(id)
}

/// Every identifier in `expr` that is not a member access (`a.B` yields `a`).
fn identifiers(expr: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < expr.len() {
        let rest = expr.get(i..).unwrap_or("");
        let Some(c) = rest.chars().next() else { break };
        if is_ident_char(c) {
            let end = rest.find(|c: char| !is_ident_char(c)).unwrap_or(rest.len());
            let prev = expr.get(..i).and_then(|p| p.trim_end().chars().last());
            if prev != Some('.')
                && let Some(id) = ident_at(rest)
            {
                out.push(id);
            }
            i += end.max(1);
        } else {
            i += c.len_utf8();
        }
    }
    out
}

/// `text[at..at + len]` is a whole word.
fn word_bounded(text: &str, at: usize, len: usize) -> bool {
    let before = text.get(..at).and_then(|s| s.chars().last());
    let after = text.get(at + len..).and_then(|s| s.chars().next());
    !before.is_some_and(is_ident_char) && !after.is_some_and(is_ident_char)
}

/// `name` is declared at any depth of `text` by `function` / `class` /
/// `const` / `let` / `var`.
fn declared_in(text: &str, name: &str) -> bool {
    ["function", "class", "const", "let", "var"].iter().any(|kw| {
        let mut from = 0;
        while let Some(rel) = text.get(from..).and_then(|s| s.find(kw)) {
            let at = from + rel;
            from = at + kw.len();
            if !word_bounded(text, at, kw.len()) {
                continue;
            }
            let rest = text
                .get(from..)
                .unwrap_or("")
                .trim_start_matches(|c: char| c.is_whitespace() || c == '*');
            if ident_at(rest) == Some(name) {
                return true;
            }
        }
        false
    })
}

/// `src` with comments and the contents of string / template literals blanked
/// to spaces (quotes and newlines kept), so a scan sees only code.
fn mask_comments(src: &str) -> Cow<'_, str> {
    let b = src.as_bytes();
    let mut out: Option<Vec<u8>> = None;
    let mut blank = |from: usize, to: usize| {
        let buf = out.get_or_insert_with(|| b.to_vec());
        for byte in buf.iter_mut().take(to).skip(from) {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            q @ (b'\'' | b'"' | b'`') => {
                let mut j = i + 1;
                while j < b.len() && b[j] != q {
                    if b[j] == b'\\' {
                        j += 1;
                    } else if b[j] == b'\n' && q != b'`' {
                        break;
                    }
                    j += 1;
                }
                let end = j.min(b.len());
                if end > i + 1 {
                    blank(i + 1, end);
                }
                i = j + 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') || b.get(i + 1) == Some(&b'*') => {
                let line = b.get(i + 1) == Some(&b'/');
                let rest = src.get(i + 2..).unwrap_or("");
                let end = if line {
                    rest.find('\n').map_or(b.len(), |k| i + 2 + k)
                } else {
                    rest.find("*/").map_or(b.len(), |k| i + 2 + k + 2)
                };
                blank(i, end);
                i = end;
            }
            _ => i += 1,
        }
    }
    match out {
        None => Cow::Borrowed(src),
        Some(v) => String::from_utf8(v).map_or(Cow::Borrowed(src), Cow::Owned),
    }
}

/// One grafted page: its route and what it adds to the file's parse.
#[derive(Debug)]
pub struct PageGraft {
    pub route: PageRoute,
    pub out: NavRouteOut,
}

/// The nav ROUTE (and HANDLED_BY ref) for one file, or `None` when the file is
/// not a page. `rel_under_root` comes from [`NextRoots::rel_under_next_root`];
/// `module_id` is the file's MODULE.
pub fn graft_page(
    rel_under_root: &str,
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> Option<PageGraft> {
    let route = page_route(rel_under_root)?;
    let rec = RouteRecord {
        path: route.path.clone(),
        handler: default_export_component(source),
        redirect: None,
        catchall: route.catchall,
    };
    let out = emit_nav_routes(&[rec], module_id, repo);
    Some(PageGraft { route, out })
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{CallQualifier, GRAPH_TYPE, cell_type, edge_category, node_kind};
    use repo_graph_core::CellPayload;

    fn route(rel: &str) -> Option<(String, bool)> {
        page_route(rel).map(|r| (r.path, r.catchall))
    }

    fn plain(p: &str) -> Option<(String, bool)> {
        Some((p.to_string(), false))
    }

    #[test]
    fn page_route_pages_router() {
        assert_eq!(route("pages/index.tsx"), plain("/"));
        assert_eq!(route("pages/about.jsx"), plain("/about"));
        assert_eq!(route("pages/users/index.ts"), plain("/users"));
        assert_eq!(route("pages/users/[id].tsx"), plain("/users/:id"));
        assert_eq!(route("pages/users/[id]/edit.js"), plain("/users/:id/edit"));
        assert_eq!(route("pages/[org]/[repo]/index.tsx"), plain("/:org/:repo"));
        assert_eq!(page_route("pages/users/[id].tsx").map(|r| r.router), Some(PageRouter::Pages));
    }

    #[test]
    fn page_route_catch_alls_keep_their_brackets() {
        assert_eq!(route("pages/docs/[...slug].tsx"), Some(("/docs/[...slug]".into(), true)));
        assert_eq!(route("pages/shop/[[...slug]].tsx"), Some(("/shop/[[...slug]]".into(), true)));
        assert_eq!(route("app/blog/[...parts]/page.tsx"), Some(("/blog/[...parts]".into(), true)));
        assert_eq!(route("pages/[[...all]].tsx"), Some(("/[[...all]]".into(), true)));
        // A catch-all must be the last segment.
        assert_eq!(route("app/[...slug]/edit/page.tsx"), None);
        // Malformed brackets are not routes.
        assert_eq!(route("pages/[].tsx"), None);
        assert_eq!(route("pages/[id.tsx"), None);
    }

    #[test]
    fn page_route_special_files_and_api_are_not_pages() {
        for rel in [
            "pages/_app.tsx",
            "pages/_document.tsx",
            "pages/_error.js",
            "pages/_middleware.ts",
            "pages/admin/_middleware.ts",
            "pages/api/health.ts",
            "pages/api/users/[id].ts",
            "pages/api/index.ts",
            "pages/users.d.ts",
            "pages/users.test.tsx",
            "pages/Button.stories.tsx",
            "pages/styles.css",
            "pages/my page.tsx",
            "app/api/users/route.ts",
            "app/layout.tsx",
            "app/orders/loading.tsx",
            "app/orders/[id]/not-found.tsx",
            "components/pages/Home.tsx",
            "lib/app/page.tsx",
        ] {
            assert_eq!(route(rel), None, "{rel}");
        }
    }

    #[test]
    fn page_route_app_router() {
        assert_eq!(route("app/page.tsx"), plain("/"));
        assert_eq!(route("app/orders/[id]/page.tsx"), plain("/orders/:id"));
        assert_eq!(route("app/(marketing)/about/page.tsx"), plain("/about"));
        assert_eq!(route("app/(shop)/(cart)/cart/page.jsx"), plain("/cart"));
        assert_eq!(route("app/@modal/login/page.tsx"), plain("/login"));
        assert_eq!(page_route("app/page.tsx").map(|r| r.router), Some(PageRouter::App));
        // Intercepting routes shadow another route; private folders opt out.
        assert_eq!(route("app/feed/(.)photo/[id]/page.tsx"), None);
        assert_eq!(route("app/feed/(..)photo/[id]/page.tsx"), None);
        assert_eq!(route("app/(...)login/page.tsx"), None);
        assert_eq!(route("app/_components/x/page.tsx"), None);
    }

    #[test]
    fn page_route_src_prefix() {
        assert_eq!(route("src/pages/index.tsx"), plain("/"));
        assert_eq!(route("src/pages/users/[id].tsx"), plain("/users/:id"));
        assert_eq!(route("src/app/orders/page.tsx"), plain("/orders"));
        assert_eq!(route("src/pages/api/x.ts"), None);
        assert_eq!(route("src/components/Home.tsx"), None);
    }

    #[test]
    fn declares_next_reads_the_three_dependency_maps() {
        assert!(declares_next(r#"{"dependencies":{"next":"14.2.0","react":"18"}}"#));
        assert!(declares_next(r#"{"devDependencies":{"next":"^13"}}"#));
        assert!(declares_next(r#"{"peerDependencies":{"next":">=12"}}"#));
        assert!(!declares_next(
            r#"{"dependencies":{"react":"18","react-router-dom":"6","vite":"5"}}"#
        ));
        // A `next` key anywhere else, or a lookalike package, is not Next.
        assert!(!declares_next(r#"{"name":"next","scripts":{"next":"next dev"}}"#));
        assert!(!declares_next(r#"{"dependencies":{"next-auth":"4"}}"#));
        assert!(!declares_next(r#"{"dependencies": {"next": "14""#));
        assert!(!declares_next(""));
    }

    #[test]
    fn next_roots_longest_owner_wins() {
        let next = r#"{"dependencies":{"next":"14"}}"#;
        let vite = r#"{"dependencies":{"vite":"5"}}"#;
        let roots = NextRoots::from_files([
            ("package.json", next),
            ("apps/spa/package.json", vite),
            ("apps/web/package.json", next),
            ("node_modules/x/package.json", next),
            ("apps/web/pages/index.tsx", ""),
        ]);
        assert_eq!(roots.next_count(), 2);
        assert_eq!(roots.rel_under_next_root("pages/index.tsx"), Some("pages/index.tsx"));
        assert_eq!(
            roots.rel_under_next_root("apps/web/pages/index.tsx"),
            Some("pages/index.tsx")
        );
        // The Vite app's pages/ folder belongs to the Vite root.
        assert_eq!(roots.rel_under_next_root("apps/spa/src/pages/Home.tsx"), None);
        // Segment-bounded: `apps/webx` is not under `apps/web`.
        assert_eq!(
            roots.rel_under_next_root("apps/webx/pages/a.tsx"),
            Some("apps/webx/pages/a.tsx")
        );

        let none = NextRoots::from_files([("package.json", vite), ("src/pages/Home.tsx", "")]);
        assert_eq!(none.next_count(), 0);
        assert_eq!(none.rel_under_next_root("src/pages/Home.tsx"), None);
        assert_eq!(NextRoots::default().rel_under_next_root("pages/a.tsx"), None);
    }

    #[test]
    fn default_export_component_forms() {
        let d = |s: &str| default_export_component(s);
        assert_eq!(d("export default function Home() { return <div/>; }"), Some("Home".into()));
        assert_eq!(d("export default async function Page() {}"), Some("Page".into()));
        assert_eq!(d("export default class Dashboard extends React.Component {}"), Some("Dashboard".into()));
        assert_eq!(d("function About() {}\nexport default About;\n"), Some("About".into()));
        assert_eq!(d("const About = () => <div/>;\nexport default About\n"), Some("About".into()));
        assert_eq!(
            d("import withAuth from '../auth';\nfunction Admin() {}\nexport default withAuth(Admin);"),
            Some("Admin".into())
        );
        assert_eq!(
            d("const Page = () => null;\nexport default connect(mapState)(Page);"),
            Some("Page".into())
        );
        assert_eq!(d("function Settings() {}\nexport { Settings as default };"), Some("Settings".into()));
    }

    #[test]
    fn default_export_component_names_nothing() {
        let d = |s: &str| default_export_component(s);
        // Anonymous function, arrow, object literal.
        assert_eq!(d("export default function () { return null; }"), None);
        assert_eq!(d("export default () => <Layout />;\nfunction Layout() {}"), None);
        assert_eq!(d("export default { title: 'x' };"), None);
        // Not declared in the file (an import), or not capitalised.
        assert_eq!(d("import Home from './Home';\nexport default Home;"), None);
        assert_eq!(d("const page = 1;\nexport default page;"), None);
        // Commented out.
        assert_eq!(d("// export default function Old() {}\nconst x = 1;"), None);
        assert_eq!(d("/* export default function Old() {} */"), None);
        // Inside a string.
        assert_eq!(d("const s = 'export default function Fake() {}';"), None);
        // Two in-file candidates: ambiguous.
        assert_eq!(d("function A() {}\nfunction B() {}\nexport default pick(A, B);"), None);
        assert_eq!(d("export const x = 1;"), None);
        assert_eq!(d(""), None);
    }

    #[test]
    fn graft_page_emits_one_nav_route_with_its_handler() {
        let repo = RepoId(7);
        let module = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "pages::users::[id]");
        let g = graft_page(
            "pages/users/[id].tsx",
            "export default function UserPage() { return <div />; }",
            module,
            repo,
        )
        .unwrap();
        assert_eq!(g.route.path, "/users/:id");
        assert_eq!(g.out.nodes.len(), 1);
        let id = g.out.nodes[0].id;
        assert_eq!(g.out.nav.qname_by_id.get(&id).map(String::as_str), Some("page:/users/:id"));
        assert_eq!(g.out.nav.kind_by_id.get(&id), Some(&node_kind::ROUTE));
        assert!(g.out.nodes[0].cells.iter().any(|c| c.kind == cell_type::ROUTE_METHOD
            && matches!(&c.payload, CellPayload::Text(t) if t == "GET")));
        assert!(g.out.nodes[0].cells.iter().any(|c| c.kind == cell_type::ORIGIN));
        assert_eq!(g.out.refs.len(), 1);
        assert_eq!(g.out.refs[0].category, edge_category::HANDLED_BY);
        assert_eq!(g.out.refs[0].from_module, module);
        assert!(matches!(&g.out.refs[0].qualifier, CallQualifier::Bare(h) if h == "UserPage"));

        // A catch-all is counted; a page with no nameable default keeps its
        // ROUTE without a HANDLED_BY.
        let c = graft_page("pages/docs/[...slug].tsx", "export default () => null;", module, repo)
            .unwrap();
        assert_eq!(c.out.catchalls, 1);
        assert!(c.out.refs.is_empty());
        assert!(graft_page("pages/_app.tsx", "export default function App() {}", module, repo).is_none());
    }
}
