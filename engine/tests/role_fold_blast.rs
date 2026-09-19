//! LB.3a — the role fold through a real engine build.
//!
//! Before the fold, `glia blast-radius angular-injects
//! users.component::UsersComponent` returned ONE hit: the SERVICE twin
//! `user.service::UserService` with no file and no line, because INJECTS bound
//! the edgeless overlay instead of the class. After it, the same query lands on
//! the located CLASS.

use std::path::PathBuf;

use repo_graph_code_domain::{cell_type, node_kind};
use repo_graph_core::{CellPayload, NodeKindId};
use repo_graph_engine::{BlastOptions, blast_radius, generate_one};
use repo_graph_graph::{MergedGraph, Reach};
use repo_graph_graph::roles::{ROLE_KINDS, roles_in};

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn build(name: &str) -> MergedGraph {
    generate_one(&fixture(name))
        .unwrap_or_else(|e| panic!("generate_one {name}: {e}"))
        .merged
}

#[test]
fn blast_radius_from_a_component_lands_on_the_located_service_class() {
    let merged = build("angular-injects");
    for direction in [Reach::Forward, Reach::Both] {
        let mut opts = BlastOptions::default();
        opts.direction = direction;
        let answer = blast_radius(&merged, &["users.component::UsersComponent"], &opts);
        assert!(answer.unresolved.is_empty(), "{direction:?}: the component resolves");
        let hits = answer.results;
        assert_eq!(
            hits.len(),
            1,
            "{direction:?}: exactly one hit, got {:?}",
            hits.iter().map(|h| (&h.kind, &h.qname)).collect::<Vec<_>>()
        );
        let hit = &hits[0];
        assert_eq!(hit.kind, "CLASS", "{direction:?}");
        assert_eq!(hit.qname, "user.service::UserService", "{direction:?}");
        assert_eq!(hit.reason, "INJECTS", "{direction:?}");
        assert_eq!(hit.file.as_deref(), Some("user.service.ts"), "{direction:?}");
        assert!(hit.line.is_some(), "{direction:?}: the class is located");
    }
}

/// The working-tree check behind the five edited `key.json`s: `grade.py`
/// reads the installed wheel, which does not carry the fold until the
/// end-of-wave rebuild. Each row is (fixture, declaration kind, name, role).
#[test]
fn edited_fixtures_carry_role_cells_and_no_twins() {
    const ROWS: &[(&str, NodeKindId, &str, NodeKindId)] = &[
        (
            "angular-injects",
            node_kind::CLASS,
            "UserService",
            node_kind::SERVICE,
        ),
        (
            "angular-injects",
            node_kind::CLASS,
            "UsersComponent",
            node_kind::COMPONENT,
        ),
        (
            "ts-angular-di",
            node_kind::CLASS,
            "ApiService",
            node_kind::SERVICE,
        ),
        (
            "ts-angular-di",
            node_kind::CLASS,
            "AppComponent",
            node_kind::COMPONENT,
        ),
        (
            "java-spring-injects",
            node_kind::CLASS,
            "UserService",
            node_kind::SERVICE,
        ),
        (
            "java-spring-injects",
            node_kind::CLASS,
            "UserController",
            node_kind::SERVICE,
        ),
        (
            "react-hook",
            node_kind::FUNCTION,
            "useCounter",
            node_kind::HOOK,
        ),
        (
            "react-hook",
            node_kind::FUNCTION,
            "Counter",
            node_kind::COMPONENT,
        ),
        (
            "angular-imports",
            node_kind::CLASS,
            "UserService",
            node_kind::SERVICE,
        ),
    ];
    let mut built: Vec<(&str, MergedGraph)> = Vec::new();
    for &(fx, kind, name, role) in ROWS {
        let at = match built.iter().position(|(f, _)| *f == fx) {
            Some(i) => i,
            None => {
                built.push((fx, build(fx)));
                built.len() - 1
            }
        };
        let merged = &built[at].1;
        let mut matches = 0;
        for g in &merged.graphs {
            for n in &g.nodes {
                let k = g.nav.kind_by_id.get(&n.id).copied();
                let nm = g.nav.name_by_id.get(&n.id).map(String::as_str);
                if nm != Some(name) {
                    continue;
                }
                assert!(
                    !k.is_some_and(|k| ROLE_KINDS.contains(&k)),
                    "{fx}: a role-kind twin of {name} survived"
                );
                if k == Some(kind) {
                    matches += 1;
                    assert!(
                        roles_in(k, &n.cells).contains(&role),
                        "{fx}: {name} lacks role {}",
                        node_kind::name(role)
                    );
                    let role_cells: Vec<&CellPayload> = n
                        .cells
                        .iter()
                        .filter(|c| c.kind == cell_type::ROLE)
                        .map(|c| &c.payload)
                        .collect();
                    assert_eq!(role_cells.len(), 1, "{fx}: {name} carries ONE ROLE cell");
                }
            }
        }
        assert_eq!(
            matches,
            1,
            "{fx}: exactly one {} {name}",
            node_kind::name(kind)
        );
    }
}
