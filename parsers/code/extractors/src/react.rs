//! React component + hook extraction.
//!
//! Runs per-file for TS/TSX/JS/JSX under React-detected projects. Emits:
//!   - COMPONENT nodes: capitalized function/const declarations returning JSX.
//!   - HOOK nodes: functions whose name starts with `use` (camelCase).
//!
//! React Router tables (`<Route path>` trees and `createBrowserRouter([...])`)
//! are the shared route-table walker's (`crate::nav_routes`, LA.6b), which
//! also covers Angular Router and vue-router.
//!
//! This is pattern-based — intentional to stay zero-AST-dependency like the
//! other cross-cutting extractors.

use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};

/// A3.4 — the ORIGIN cell that marks a ROUTE node as a *browser navigation*
/// target rather than a server endpoint.
///
/// react-router / Angular Router / vue-router all mint a ROUTE with qname
/// `GET <path>`, which `index_route_node`'s legacy branch cannot tell apart
/// from a Spring / Express / Rails route — so a same-repo `fetch('/dashboard')`
/// pairs to the SPA's own navigation table and HttpStackResolver emits a false
/// `HTTP_CALLS` edge. The node itself is legitimate ("where is /dashboard
/// rendered?"), so we MARK it rather than dropping it; the graph crate's
/// `is_nav_route` reads this cell and skips the node when building the HTTP
/// route index.
///
/// Reuses the existing `cell_type::ORIGIN` with an additional `provenance`
/// value — the provenance vocabulary is already open-ended (the doc pipeline
/// writes `"provenance":"documentation"`), so no new CellType id is minted.
///
/// Canonical for the whole extractors crate: `nav_routes` (Angular / React /
/// Vue route tables, LA.6b) imports this rather than re-spelling the payload,
/// since the graph-side matcher is a literal substring test on exactly this
/// text.
pub(crate) fn nav_route_origin_cell() -> Cell {
    Cell {
        kind: cell_type::ORIGIN,
        payload: CellPayload::Json(r#"{"provenance":"nav_route"}"#.to_string()),
    }
}

pub struct ReactNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
}

pub fn extract_react_nodes(
    source: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
) -> ReactNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();

    // --- Components: capitalized function/const whose body has JSX-ish marker.
    for name in scan_component_names(source) {
        let qname = format!("{module_qname}::{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::COMPONENT, &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        nav.record(id, &name, &qname, node_kind::COMPONENT, Some(module_id));
    }

    // --- Hooks: `use<Xxx>` function / const arrow declarations.
    for name in scan_hook_names(source) {
        let qname = format!("{module_qname}::{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::HOOK, &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        nav.record(id, &name, &qname, node_kind::HOOK, Some(module_id));
    }

    ReactNodes { nodes, nav }
}

fn scan_component_names(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let has_jsx = source.contains("</") || source.contains("/>");
    if !has_jsx {
        return out;
    }
    for line in source.lines() {
        let t = line.trim_start();
        // function Foo(
        if let Some(rest) = t.strip_prefix("export default function ") {
            if let Some(name) = take_ident(rest) {
                if is_capitalized(&name) {
                    out.push(name);
                }
            }
        } else if let Some(rest) = t.strip_prefix("export function ") {
            if let Some(name) = take_ident(rest) {
                if is_capitalized(&name) {
                    out.push(name);
                }
            }
        } else if let Some(rest) = t.strip_prefix("function ") {
            if let Some(name) = take_ident(rest) {
                if is_capitalized(&name) {
                    out.push(name);
                }
            }
        } else if let Some(rest) = t.strip_prefix("export const ") {
            if let Some(name) = take_ident(rest) {
                if is_capitalized(&name)
                    && (line.contains("=>") || line.contains("React.FC") || line.contains(": FC"))
                {
                    out.push(name);
                }
            }
        } else if let Some(rest) = t.strip_prefix("const ") {
            if let Some(name) = take_ident(rest) {
                if is_capitalized(&name)
                    && (line.contains("=>") || line.contains("React.FC") || line.contains(": FC"))
                {
                    out.push(name);
                }
            }
        }
    }
    dedup(out)
}

fn scan_hook_names(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        let candidate = if let Some(r) = t.strip_prefix("export function ") {
            take_ident(r)
        } else if let Some(r) = t.strip_prefix("function ") {
            take_ident(r)
        } else if let Some(r) = t.strip_prefix("export const ") {
            take_ident(r).filter(|_| line.contains("=>"))
        } else if let Some(r) = t.strip_prefix("const ") {
            take_ident(r).filter(|_| line.contains("=>"))
        } else {
            None
        };
        if let Some(name) = candidate {
            if is_hook_name(&name) {
                out.push(name);
            }
        }
    }
    dedup(out)
}

fn take_ident(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
    {
        i += 1;
    }
    if i == 0 {
        None
    } else {
        Some(s[..i].to_string())
    }
}

fn is_capitalized(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

fn is_hook_name(s: &str) -> bool {
    if !s.starts_with("use") || s.len() < 4 {
        return false;
    }
    // `use<X>` where X is uppercase.
    s.chars().nth(3).is_some_and(|c| c.is_ascii_uppercase())
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
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
    fn detects_function_component() {
        let src = r#"
export function UserCard({ user }: Props) {
    return <div>{user.name}</div>;
}
"#;
        let r = extract_react_nodes(src, "test", module_id(), repo());
        let comps: Vec<_> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::COMPONENT)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id))
            .collect();
        assert!(comps.iter().any(|n| n.as_str() == "UserCard"));
    }

    #[test]
    fn detects_arrow_component() {
        let src = "export const Button = ({ label }) => (<button>{label}</button>);";
        let r = extract_react_nodes(src, "test", module_id(), repo());
        assert!(r.nav.name_by_id.values().any(|n| n == "Button"));
    }

    #[test]
    fn detects_hook() {
        let src = "export function useAuth() { return useContext(AuthCtx); }";
        let r = extract_react_nodes(src, "test", module_id(), repo());
        let hooks: Vec<_> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::HOOK)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id))
            .collect();
        assert!(hooks.iter().any(|n| n.as_str() == "useAuth"));
    }

    #[test]
    fn detects_react_router_jsx() {
        let src = r#"
<Routes>
  <Route path="/users" element={<Users />} />
  <Route path="/users/:id" element={<UserDetail />} />
</Routes>
"#;
        // LA.6b: React Router tables are the shared walker's; the React
        // extractor itself mints no ROUTE any more.
        let own = extract_react_nodes(src, "test", module_id(), repo());
        assert!(own.nav.kind_by_id.values().all(|k| *k != node_kind::ROUTE));
        let r = crate::nav_routes::extract_route_tables(src, module_id(), repo());
        let names: Vec<&str> = r
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| r.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(names.contains(&"/users"));
        assert!(names.contains(&"/users/:id"));
        // LB.4c: a nav page lives in the `page:<path>` qname namespace, its
        // display name is the bare path.
        let mut qnames: Vec<&str> = r
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, _)| r.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, q)| q.as_str())
            .collect();
        qnames.sort_unstable();
        assert_eq!(qnames, ["page:/users", "page:/users/:id"]);
        // LB.4c: the page's NodeId is NOT the one a same-repo server route
        // `GET /users` (Flask, Spring, Express, ...) hashes to — before LB.4c
        // they were one NodeId stored in two per-language graphs.
        let page = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, "page:/users");
        let server = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, "GET /users");
        assert!(r.nodes.iter().any(|n| n.id == page));
        assert!(r.nodes.iter().all(|n| n.id != server));
        assert_eq!(r.nav_routes, 2, "A3.4: every client-router ROUTE counted");
        // LA.6b: `element={<X />}` binds the route to its page component.
        assert_eq!(
            handled_by(&r),
            [("page:/users", "Users"), ("page:/users/:id", "UserDetail")]
        );
        // A3.4: each one carries the `provenance: nav_route` ORIGIN mark, which
        // is what `graph::nav::is_nav_route` reads to keep it out of
        // the HTTP pairing index.
        let route_ids: Vec<_> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(route_ids.len(), 2);
        for id in route_ids {
            let node = r.nodes.iter().find(|n| n.id == id).expect("route node");
            assert!(
                node.cells.iter().any(|c| c.kind == cell_type::ORIGIN
                    && matches!(&c.payload, CellPayload::Json(j)
                                if j.contains("\"provenance\":\"nav_route\""))),
                "nav route must carry the ORIGIN provenance mark"
            );
        }
    }

    #[test]
    fn detects_router_object_form() {
        let src = r#"
createBrowserRouter([
    { path: '/', element: <Home /> },
    { path: '/about', element: <About /> },
]);
"#;
        // LA.6b: React Router tables are the shared walker's; the React
        // extractor itself mints no ROUTE any more.
        let own = extract_react_nodes(src, "test", module_id(), repo());
        assert!(own.nav.kind_by_id.values().all(|k| *k != node_kind::ROUTE));
        let r = crate::nav_routes::extract_route_tables(src, module_id(), repo());
        let names: Vec<&str> = r
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| r.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(names.contains(&"/"));
        assert!(names.contains(&"/about"));
        // LB.4c: a nav page lives in the `page:<path>` qname namespace, its
        // display name is the bare path.
        let mut qnames: Vec<&str> = r
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, _)| r.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, q)| q.as_str())
            .collect();
        qnames.sort_unstable();
        assert_eq!(qnames, ["page:/", "page:/about"]);
        assert_eq!(
            handled_by(&r),
            [("page:/", "Home"), ("page:/about", "About")]
        );
    }

    /// `(route qname, handler)` for every LA.6b HANDLED_BY ref, sorted.
    fn handled_by(r: &crate::nav_routes::NavRouteOut) -> Vec<(&str, &str)> {
        use glia_code_domain::{CallQualifier, edge_category};
        let mut out: Vec<(&str, &str)> = r
            .refs
            .iter()
            .filter(|x| x.category == edge_category::HANDLED_BY)
            .filter_map(|x| match &x.qualifier {
                CallQualifier::Bare(h) => {
                    Some((r.nav.qname_by_id.get(&x.from)?.as_str(), h.as_str()))
                }
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn lowercase_function_not_component() {
        let src = "function helper() { return <div />; }";
        let r = extract_react_nodes(src, "test", module_id(), repo());
        assert!(
            r.nodes
                .iter()
                .all(|n| { r.nav.kind_by_id.get(&n.id) != Some(&node_kind::COMPONENT) })
        );
    }
}
