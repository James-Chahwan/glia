//! LA.6c — Angular `.component.html` templates are walked and their links
//! become `NAVIGATES_TO` refs of the component's page.
//!
//! A template shares its MODULE qname with its `.component.ts`
//! (`path_to_qname` strips only the last extension), so its refs carry the
//! component module's id and LA.6a lifts them onto the page component. The
//! template mints no node of its own. Route paths are compared through
//! `nav::nav_route_path`, never a ROUTE qname literal, so the test survives a
//! change of the nav qname shape.

use repo_graph_code_domain::{CallQualifier, edge_category, node_kind};
use repo_graph_core::NodeId;
use repo_graph_engine::generate_one;
use repo_graph_graph::RepoGraph;
use repo_graph_graph::nav::nav_route_path;
use repo_graph_graph::roles::roles_in;

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

/// Every (graph, node) pair `keep` accepts, in graph and node order.
fn find(
    graphs: &[RepoGraph],
    keep: impl Fn(&RepoGraph, NodeId) -> bool,
) -> Vec<(&RepoGraph, NodeId)> {
    graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(move |n| (g, n.id)))
        .filter(|(g, id)| keep(g, *id))
        .collect()
}

#[test]
fn component_template_links_are_the_page_s_navigation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "src/app/app.routes.ts",
        "import { Routes } from '@angular/router';\n\
         import { HomeComponent } from './home/home.component';\n\
         \n\
         export const routes: Routes = [\n  { path: 'home', component: HomeComponent },\n];\n",
    );
    write(
        root,
        "src/app/home/home.component.ts",
        "import { Component } from '@angular/core';\n\
         \n\
         @Component({ selector: 'app-home', templateUrl: './home.component.html' })\n\
         export class HomeComponent {}\n",
    );
    write(
        root,
        "src/app/home/home.component.html",
        "<a routerLink=\"/home\">Home</a>\n\
         <a routerLink=\"/nowhere\">Nowhere</a>\n\
         <img src=\"/logo.png\">\n\
         <a href=\"/favicon.ico\">icon</a>\n",
    );
    // A plain page is not an Angular template: never walked.
    write(root, "src/index.html", "<a href=\"/elsewhere\">x</a>\n");

    let merged = generate_one(&root.to_string_lossy())
        .expect("generate_one")
        .merged;
    let graphs = &merged.graphs;

    let pages = find(graphs, |g, id| {
        g.nav
            .qname_by_id
            .get(&id)
            .is_some_and(|q| q.ends_with("home.component::HomeComponent"))
            && g.nodes.iter().find(|n| n.id == id).is_some_and(|n| {
                roles_in(g.nav.kind_by_id.get(&id).copied(), &n.cells)
                    .contains(&node_kind::COMPONENT)
            })
    });
    assert_eq!(pages.len(), 1, "one HomeComponent page node");
    let (g, page) = pages[0];

    let home_routes: Vec<NodeId> = g
        .nodes
        .iter()
        .map(|n| n.id)
        .filter(|id| g.nav.kind_by_id.get(id) == Some(&node_kind::ROUTE))
        .filter(|id| g.nav.qname_by_id.get(id).and_then(|q| nav_route_path(q)) == Some("/home"))
        .collect();
    assert_eq!(home_routes.len(), 1, "one nav ROUTE serves /home");

    let nav_edges: Vec<(NodeId, NodeId)> = graphs
        .iter()
        .flat_map(|g| &g.edges)
        .filter(|e| e.category == edge_category::NAVIGATES_TO)
        .map(|e| (e.from, e.to))
        .collect();
    assert_eq!(
        nav_edges,
        [(page, home_routes[0])],
        "exactly the template's routerLink=\"/home\", lifted to the page"
    );

    let nav_refs: Vec<(NodeId, &str)> = graphs
        .iter()
        .flat_map(|g| &g.unresolved_refs)
        .filter(|r| r.category == edge_category::NAVIGATES_TO)
        .map(|r| match &r.qualifier {
            CallQualifier::Bare(s) => (r.from, s.as_str()),
            other => panic!("a NAVIGATES_TO ref is Bare, got {other:?}"),
        })
        .collect();
    assert_eq!(
        nav_refs,
        [(page, "/nowhere")],
        "the dead link is kept, from the page; the asset paths are never links"
    );

    let template_modules = find(graphs, |g, id| {
        g.nav.kind_by_id.get(&id) == Some(&node_kind::MODULE)
            && g.nav
                .qname_by_id
                .get(&id)
                .is_some_and(|q| q == "src::app::home::home.component")
    });
    assert_eq!(
        template_modules.len(),
        1,
        "the template shares the component's MODULE and mints none of its own"
    );
    assert!(
        !graphs
            .iter()
            .any(|g| g.nav.qname_by_id.values().any(|q| q == "src::index")),
        "index.html is never walked"
    );
}
