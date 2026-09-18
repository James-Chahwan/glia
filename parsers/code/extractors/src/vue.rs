//! Vue component/composable extraction.
//!
//! Pattern-based. Runs per-file on TS under Vue-detected projects. Emits:
//!   - COMPONENT: one per `.vue` file (name from basename) + `defineComponent({...})`.
//!   - COMPOSABLE: `export function useX` / `export const useX = (...) =>`.
//!
//! vue-router tables (`{ path: '/x', component: X, children: [...] }`) are
//! the shared route-table walker's (`crate::nav_routes`, LA.6b).

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Confidence, Node, NodeId, RepoId};

pub struct VueNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
}

pub fn extract_vue_nodes(
    source: &str,
    path: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
) -> VueNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();

    // --- Component: one per .vue file, keyed by basename.
    if path.ends_with(".vue") {
        if let Some(name) = vue_component_name_from_path(path) {
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
    }
    // `defineComponent` usage (in any .ts / .vue <script> content).
    for name in scan_define_component_names(source) {
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

    // --- Composables: `useX` function/const declarations.
    for name in scan_composable_names(source) {
        let qname = format!("{module_qname}::{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::COMPOSABLE, &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        nav.record(id, &name, &qname, node_kind::COMPOSABLE, Some(module_id));
    }

    VueNodes { nodes, nav }
}

fn vue_component_name_from_path(path: &str) -> Option<String> {
    let norm = path.replace('\\', "/");
    let filename = norm.rsplit('/').next()?;
    let stem = filename.strip_suffix(".vue")?;
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

fn scan_define_component_names(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        // `export default defineComponent({...})` → emit component named after
        // file basename — skip here since we already emit from path.
        // Pattern of interest: `const X = defineComponent({...})` or
        // `export const X = defineComponent(`
        if let Some(rest) = t.strip_prefix("export const ") {
            if let Some(name) = take_ident(rest) {
                if line.contains("defineComponent(") {
                    out.push(name);
                }
            }
        } else if let Some(rest) = t.strip_prefix("const ") {
            if let Some(name) = take_ident(rest) {
                if line.contains("defineComponent(") {
                    out.push(name);
                }
            }
        }
    }
    dedup(out)
}

fn scan_composable_names(source: &str) -> Vec<String> {
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
            if is_composable_name(&name) {
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

fn is_composable_name(s: &str) -> bool {
    if !s.starts_with("use") || s.len() < 4 {
        return false;
    }
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
    use repo_graph_code_domain::{CallQualifier, cell_type, edge_category};
    use repo_graph_core::CellPayload;

    fn repo() -> RepoId {
        RepoId(1)
    }
    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test")
    }

    #[test]
    fn component_from_vue_path() {
        let r = extract_vue_nodes(
            "",
            "src/components/UserCard.vue",
            "test",
            module_id(),
            repo(),
        );
        let names: Vec<&str> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::COMPONENT)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(names.contains(&"UserCard"));
    }

    #[test]
    fn composable_detected() {
        let src = "export function useAuth() { return {} }";
        let r = extract_vue_nodes(
            src,
            "src/composables/useAuth.ts",
            "test",
            module_id(),
            repo(),
        );
        let names: Vec<&str> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::COMPOSABLE)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(names.contains(&"useAuth"));
    }

    #[test]
    fn router_routes() {
        let src = r#"
const routes = [
    { path: '/', component: Home },
    { path: '/users', component: Users },
    { path: '/users/:id', component: UserDetail },
];
createRouter({ history: createWebHistory(), routes });
"#;
        // LA.6b: vue-router tables are the shared walker's; the Vue extractor
        // itself mints no ROUTE any more.
        let own = extract_vue_nodes(src, "src/router.ts", "test", module_id(), repo());
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
        assert_eq!(qnames, ["page:/", "page:/users", "page:/users/:id"]);
        assert_eq!(r.nav_routes, 3, "A3.4: every client-router ROUTE counted");
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
        assert_eq!(route_ids.len(), 3);
        for id in route_ids {
            let node = r.nodes.iter().find(|n| n.id == id).expect("route node");
            assert!(
                node.cells.iter().any(|c| c.kind == cell_type::ORIGIN
                    && matches!(&c.payload, CellPayload::Json(j)
                                if j.contains("\"provenance\":\"nav_route\""))),
                "nav route must carry the ORIGIN provenance mark"
            );
        }
        // LA.6b: `component: X` binds each route to its page component.
        let mut bound: Vec<(&str, &str)> = r
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
        bound.sort_unstable();
        assert_eq!(
            bound,
            [
                ("page:/", "Home"),
                ("page:/users", "Users"),
                ("page:/users/:id", "UserDetail")
            ]
        );
    }

    #[test]
    fn define_component_const() {
        let src = "export const UserCard = defineComponent({ props: {} });";
        let r = extract_vue_nodes(src, "src/UserCard.ts", "test", module_id(), repo());
        let names: Vec<&str> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::COMPONENT)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(names.contains(&"UserCard"));
    }
}
