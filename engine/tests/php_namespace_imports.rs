//! DF.2: a PHP symbol import binds the member of the namespace it names,
//! proven on a real build of the `php-use-qualified-binding` and
//! `php-use-unique-wrong-namespace` fixtures (copied into a tempdir, so the
//! build writes nothing under `bench/`).
//!
//! A PSR-4 class is directory-scoped (`src::Billing::Invoicer`, LB.7b) while
//! its `use` names the namespace (`App\Billing\Invoicer`), so no MODULE qname
//! matches the import and HEAD fell back to the repo-unique short name: two
//! classes named `Invoicer` bound neither import, and one bound even
//! `use App\Legal\Invoicer`, a namespace no file declares. The graph import
//! pass now binds through the namespace PACKAGE each file opens (rule
//! `namespace`, `[imports] namespace symbols:` on stderr) and refuses a
//! `.php` import into an undeclared namespace under a root the repo owns.

use std::collections::HashMap;
use std::path::Path;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::generate_one;

/// One edge as `(from kind, from qname, to kind, to qname, evidence rule)`.
type Row = (u32, String, u32, String, String);

/// Copy every source file of fixture `name` (not its key.json) into a fresh
/// tempdir.
fn copy_fixture(name: &str) -> tempfile::TempDir {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures").join(name);
    let dst = tempfile::tempdir().expect("tempdir");
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("fixture dir") {
            let path = entry.expect("dir entry").path();
            let rel = path.strip_prefix(&src).expect("under the fixture");
            if path.is_dir() {
                std::fs::create_dir_all(dst.path().join(rel)).expect("mkdir");
                stack.push(path);
            } else if rel != Path::new("key.json") {
                std::fs::copy(&path, dst.path().join(rel)).expect("copy");
            }
        }
    }
    dst
}

/// Every edge of `category` in a build of fixture `name`, sorted.
fn edges(name: &str, category: EdgeCategoryId) -> Vec<Row> {
    let dir = copy_fixture(name);
    let r = generate_one(dir.path().to_str().expect("utf-8 tempdir")).expect("fixture builds");
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
            let rule = Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default();
            (k(&e.from), q(&e.from), k(&e.to), q(&e.to), rule)
        })
        .collect();
    out.sort();
    out
}

/// The rule of the one edge `from -> to`, if there is one.
fn rule_of(rows: &[Row], from: (NodeKindId, &str), to: (NodeKindId, &str)) -> Option<String> {
    let mut hits = rows.iter().filter(|(fk, f, tk, t, _)| {
        (*fk, f.as_str()) == (from.0.0, from.1) && (*tk, t.as_str()) == (to.0.0, to.1)
    });
    let hit = hits.next().map(|r| r.4.clone());
    assert!(hits.next().is_none(), "one edge {from:?} -> {to:?}; rows = {rows:?}");
    hit
}

fn targets_of<'a>(rows: &'a [Row], from: &str) -> Vec<&'a str> {
    rows.iter().filter(|r| r.1 == from).map(|r| r.3.as_str()).collect()
}

const ORDER: &str = "src::Orders::OrderService";
const SHIPMENT: &str = "src::Orders::ShipmentService";
const LEGAL: &str = "src::Orders::LegalService";
const BILLING: &str = "src::Billing::Invoicer";
const SHIPPING: &str = "src::Shipping::Invoicer";

#[test]
fn two_invoicers_each_bind_their_own_namespace() {
    let imports = edges("php-use-qualified-binding", edge_category::IMPORTS);
    let m = node_kind::MODULE;
    let c = node_kind::CLASS;
    assert_eq!(rule_of(&imports, (m, ORDER), (c, BILLING)).as_deref(), Some("namespace"), "{imports:?}");
    assert_eq!(rule_of(&imports, (m, SHIPMENT), (c, SHIPPING)).as_deref(), Some("namespace"), "{imports:?}");
    assert_eq!(targets_of(&imports, ORDER), [BILLING], "{imports:?}");
    assert_eq!(targets_of(&imports, SHIPMENT), [SHIPPING], "{imports:?}");
    // `use App\Legal\Invoicer`: no file declares App\Legal.
    assert!(targets_of(&imports, LEGAL).is_empty(), "{imports:?}");

    let calls = edges("php-use-qualified-binding", edge_category::CALLS);
    let fq = node_kind::METHOD;
    let place = (fq, "src::Orders::OrderService::place");
    let ship = (fq, "src::Orders::ShipmentService::ship");
    assert!(rule_of(&calls, place, (fq, "src::Billing::Invoicer::issue")).is_some(), "{calls:?}");
    assert!(rule_of(&calls, ship, (fq, "src::Shipping::Invoicer::issue")).is_some(), "{calls:?}");
    assert!(rule_of(&calls, ship, (fq, "src::Billing::Invoicer::issue")).is_none(), "{calls:?}");
    assert!(targets_of(&calls, "src::Orders::LegalService::check").is_empty(), "{calls:?}");
}

#[test]
fn an_undeclared_namespace_under_the_repo_root_binds_nothing() {
    let imports = edges("php-use-unique-wrong-namespace", edge_category::IMPORTS);
    assert!(targets_of(&imports, LEGAL).is_empty(), "HEAD's tail guess bound Billing's; {imports:?}");
    // The control: the declared namespace binds, now by rule `namespace`.
    assert_eq!(
        rule_of(&imports, (node_kind::MODULE, ORDER), (node_kind::CLASS, BILLING)).as_deref(),
        Some("namespace"),
        "{imports:?}"
    );
    let calls = edges("php-use-unique-wrong-namespace", edge_category::CALLS);
    assert!(targets_of(&calls, "src::Orders::LegalService::check").is_empty(), "{calls:?}");
    assert_eq!(
        targets_of(&calls, "src::Orders::OrderService::place"),
        ["src::Billing::Invoicer::issue"],
        "{calls:?}"
    );
}
