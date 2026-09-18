//! LA.40b: a PHP `use` inside the braced `namespace X { }` form binds for its
//! FILE, like a statement-form `use`, proven on a real build of the
//! `php-braced-use` fixture.
//!
//! Before LA.40b the parser recorded the braced body's `use` with the namespace
//! qname (`App::Http::Controllers`) as `from_module`; the graph's import pass
//! looks `from_module` up among MODULE nodes only, found none, and dropped the
//! statement: no IMPORTS edge, no `User` binding, so the controller's
//! `$u = new User(); $u->find($id)` never resolved. `app/Legacy/Report.php`
//! (statement form) is the control. Run with `GLIA_PHP_DEBUG=1` and
//! `-- --nocapture` to see the `[php-use]` marker.

use std::collections::HashMap;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_engine::generate_one;

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/php-braced-use"
    )
    .to_string()
}

/// One edge as `(from kind id, from qname, to kind id, to qname)`.
type Row = (u32, String, u32, String);

/// Every edge of `category` in the build, sorted.
fn edges(category: EdgeCategoryId) -> Vec<Row> {
    let r = generate_one(&fixture()).expect("fixture builds");
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    let mut kind: HashMap<NodeId, u32> = HashMap::new();
    for g in &r.merged.graphs {
        qname.extend(g.nav.qname_by_id.iter().map(|(id, q)| (*id, q.clone())));
        kind.extend(g.nav.kind_by_id.iter().map(|(id, k)| (*id, k.0)));
    }
    let mut out: Vec<Row> = r
        .merged
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| {
            let q = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
            let k = |id: &NodeId| kind.get(id).copied().unwrap_or(u32::MAX);
            (k(&e.from), q(&e.from), k(&e.to), q(&e.to))
        })
        .collect();
    out.sort();
    out
}

fn has(rows: &[Row], from: (NodeKindId, &str), to: (NodeKindId, &str)) -> bool {
    rows.iter().any(|(fk, f, tk, t)| {
        (*fk, f.as_str()) == (from.0.0, from.1) && (*tk, t.as_str()) == (to.0.0, to.1)
    })
}

#[test]
fn braced_namespace_use_imports_from_the_file_module() {
    let imports = edges(edge_category::IMPORTS);
    assert!(
        has(
            &imports,
            (node_kind::MODULE, "app::Http::Controllers::UserController"),
            (node_kind::CLASS, "App::Models::User"),
        ),
        "the braced-namespace use must import from the file MODULE; IMPORTS = {imports:?}"
    );
    assert!(
        has(
            &imports,
            (node_kind::MODULE, "app::Legacy::Report"),
            (node_kind::CLASS, "App::Models::User"),
        ),
        "control: the statement-form use; IMPORTS = {imports:?}"
    );
    assert!(
        !imports.iter().any(|(fk, ..)| *fk == node_kind::PACKAGE.0),
        "an import never belongs to a namespace PACKAGE; IMPORTS = {imports:?}"
    );
}

#[test]
fn calls_binding_through_the_braced_use_resolve() {
    let calls = edges(edge_category::CALLS);
    assert!(
        has(
            &calls,
            (
                node_kind::METHOD,
                "App::Http::Controllers::UserController::show"
            ),
            (node_kind::METHOD, "App::Models::User::find"),
        ),
        "`new User()` binds through the braced use; CALLS = {calls:?}"
    );
    assert!(
        has(
            &calls,
            (node_kind::METHOD, "app::Legacy::Report::run"),
            (node_kind::METHOD, "App::Models::User::find"),
        ),
        "control: the statement form; CALLS = {calls:?}"
    );
}
