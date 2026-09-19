//! End-to-end render of the `http_stack_smoke/backend` Go graph through the
//! dense text projection. Asserts the output structure (LEGEND, TOPOLOGY,
//! per-node blocks) matches what the format spec promises and that the real
//! graph contents — Routes, handler functions, CALLS and HANDLED_BY edges —
//! flow into the right sigils.

use std::path::PathBuf;

use glia_core::RepoId;
use glia_graph::build_go;
use glia_parser_go::parse_file;
use glia_projection_text::render_repo_graph;

const MODULE_PREFIX: &str = "example.com/backend";

fn backend_root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent()
        .unwrap()
        .join("tests/fixtures/http_stack_smoke/backend")
}

fn repo() -> RepoId {
    RepoId::from_canonical("test://http_stack_smoke/backend")
}

fn build() -> glia_graph::RepoGraph {
    let files = [
        ("users/users.go", "users"),
        ("server/server.go", "server"),
    ];
    let parses: Vec<_> = files
        .iter()
        .map(|(rel, pkg)| {
            let src = std::fs::read_to_string(backend_root().join(rel)).unwrap();
            parse_file(&src, rel, pkg, MODULE_PREFIX, repo()).unwrap()
        })
        .collect();
    build_go(repo(), parses).unwrap()
}

#[test]
fn backend_renders_with_legend_topology_and_node_blocks() {
    let g = build();
    let out = render_repo_graph(&g);

    // Top-level structure.
    assert!(out.starts_with("[LEGEND]"), "missing LEGEND header:\n{out}");
    assert!(out.contains("[TOPOLOGY]"), "missing TOPOLOGY header:\n{out}");

    // Routes are entry kinds — their topology lines must carry the `*` sigil.
    let topology_section = out
        .split("[TOPOLOGY]")
        .nth(1)
        .and_then(|s| s.split("\n[").next())
        .unwrap_or("");
    assert!(
        topology_section.contains("GET /api/users") && topology_section.contains(" * > "),
        "expected at least one starred route line in topology:\n{topology_section}"
    );

    // Per-node block headers exist for the Routes the parser extracted, one
    // per (method, path) (LB.11a).
    assert!(
        out.contains("[GET /api/users]"),
        "missing GET /api/users route block:\n{out}"
    );
    // Route qnames preserve the original path syntax (`:id`); normalisation
    // only happens inside `HttpStackResolver` for cross-repo matching.
    assert!(
        out.contains("[GET /api/users/:id]"),
        "missing GET /api/users/:id route block:\n{out}"
    );

    // The handler functions appear too — modules-as-Go-packages plus their
    // top-level funcs. Go qnames use simple `package::Name` form.
    assert!(
        out.contains("[users::List]"),
        "missing handler function block:\n{out}"
    );

    // HANDLED_BY edges from Routes to handler funcs land in topology.
    assert!(
        out.contains("GET /api/users * > users::List"),
        "missing route → handler topology line:\n{out}"
    );

    // Confidence and kind labels render on every node block.
    assert!(out.contains(":kind       Route"));
    assert!(out.contains(":kind       Function"));
    assert!(out.contains(":confidence strong"));
}
