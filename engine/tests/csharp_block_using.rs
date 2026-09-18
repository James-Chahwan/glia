//! LA.40a: a C# `using` written inside a block `namespace X { }` (StyleCop
//! SA1200's default placement) imports from its FILE, like a file-level using,
//! proven on a real build of the `csharp-block-using` fixture.
//!
//! Before LA.40a the parser recorded the block body's using with the namespace
//! qname (`Shop::Controllers`) as `from_module`; the graph's import pass looks
//! `from_module` up among MODULE nodes only, found none, and dropped the
//! statement: no IMPORTS edge from the file. `Services/Invoices.cs` (a using at
//! the top of the file) is the control. Run with `GLIA_CSHARP_DEBUG=1` and
//! `-- --nocapture` to see the `[csharp-using]` marker.

use std::collections::HashMap;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{NodeId, NodeKindId};
use repo_graph_engine::generate_one;

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/csharp-block-using"
    )
    .to_string()
}

/// One IMPORTS edge as `(from kind id, from qname, to kind id, to qname)`.
type Row = (u32, String, u32, String);

/// Every IMPORTS edge of the build, sorted.
fn imports() -> Vec<Row> {
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
        .filter(|e| e.category == edge_category::IMPORTS)
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
fn block_namespace_usings_import_from_the_file_module() {
    let rows = imports();
    let controller = (node_kind::MODULE, "Controllers::UserController");
    assert!(
        has(&rows, controller, (node_kind::PACKAGE, "Shop::Models")),
        "the block-namespace using must import from the file MODULE; IMPORTS = {rows:?}"
    );
    assert!(
        has(&rows, controller, (node_kind::PACKAGE, "Shop::Services::Billing")),
        "the second block-namespace using; IMPORTS = {rows:?}"
    );
    assert!(
        has(
            &rows,
            (node_kind::MODULE, "Services::Invoices"),
            (node_kind::PACKAGE, "Shop::Models"),
        ),
        "control: the file-level using; IMPORTS = {rows:?}"
    );
    assert!(
        !rows.iter().any(|(fk, ..)| *fk == node_kind::PACKAGE.0),
        "an import never belongs to a namespace PACKAGE; IMPORTS = {rows:?}"
    );
}
