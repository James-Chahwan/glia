//! LB.3b — a folded framework role seeds liveness.
//!
//! After the LB.3a fold an Angular `@Component` is a CLASS carrying a ROLE
//! cell, not a COMPONENT node, so an entrypoint test on kind + name alone
//! stopped seeding it and everything it injects read `live: false`. The
//! entrypoint test now also reads the node's roles through
//! `repo_graph_graph::roles::roles_in`.

use std::path::PathBuf;

use repo_graph_code_domain::node_kind;
use repo_graph_engine::{blast_radius_by_qname, entrypoint_reachable, generate_one};
use repo_graph_graph::MergedGraph;

fn build(name: &str) -> MergedGraph {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name);
    generate_one(&dir.to_string_lossy())
        .unwrap_or_else(|e| panic!("generate_one {name}: {e}"))
        .merged
}

#[test]
fn component_role_seeds_liveness() {
    let merged = build("angular-injects");

    let hits = blast_radius_by_qname(
        &merged,
        "users.component::UsersComponent",
        "forward",
        4,
        None,
        false,
        None,
    )
    .expect("blast radius");
    let service = hits
        .iter()
        .find(|h| h.qname == "user.service::UserService")
        .unwrap_or_else(|| {
            panic!(
                "UserService is in the radius, got {:?}",
                hits.iter().map(|h| (h.kind, &h.qname)).collect::<Vec<_>>()
            )
        });
    assert_eq!(service.kind, "CLASS");
    assert!(
        service.live,
        "UserService is injected by a COMPONENT-role class, so it is live"
    );

    let component = merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(move |n| (g, n)))
        .find(|(g, n)| {
            g.nav.qname_by_id.get(&n.id).map(String::as_str)
                == Some("users.component::UsersComponent")
                && g.nav.kind_by_id.get(&n.id) == Some(&node_kind::CLASS)
        })
        .map(|(_, n)| n.id)
        .expect("the folded UsersComponent CLASS");
    assert!(
        entrypoint_reachable(&merged).contains(&component),
        "a CLASS carrying ROLE COMPONENT is an entrypoint"
    );
}
