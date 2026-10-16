//! DD.16 (Q16): the identity resolver's SAME_AS stars are the only join of a
//! package or a message across repos. The `package` resolver's
//! SHARES_DEPENDENCY clique and the `message_schema` resolver's MESSAGE_TYPE
//! SHARES_SCHEMA pairs are retired with both resolvers (their `glia_graph`
//! types are gone); SharedSchemaResolver's DTO SHARES_SCHEMA pairs (sharing, not
//! identity) stay. Real builds of the substrate-gap fixtures (`generate_many`
//! on their dirs).
//!
//! Baseline before DD.16 (HEAD 9c7c6faa): identity-npm-package had 2
//! SHARES_DEPENDENCY edges beside its 2 SAME_AS stars, identity-proto-diverged
//! the 3-edge SHARES_SCHEMA clique beside its 2 SAME_AS stars, and every build
//! with a MESSAGE_TYPE printed a `[schema-link]` line.
//!
//! Liveness is untouched: the only edges an extractor emits INTO a
//! MESSAGE_TYPE are DEFINES from its schema MODULE, which is no carry edge, so
//! no MESSAGE_TYPE was ever live and the removed pairs carried nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, node_kind, same_as};
use glia_core::{EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::contract_fields::contract_fields;
use glia_engine::{GenerateResult, entrypoint_reachable, generate_many};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
}

fn dirs(name: &str, dirs: &[&str]) -> Vec<String> {
    let root = fixture(name);
    dirs.iter()
        .map(|d| root.join(d).to_string_lossy().into_owned())
        .collect()
}

fn build(name: &str, ds: &[&str]) -> GenerateResult {
    generate_many(&dirs(name, ds)).unwrap_or_else(|e| panic!("{name} builds: {e}"))
}

/// Every node's kind and qname, from the first graph naming it.
fn nav(r: &GenerateResult) -> HashMap<NodeId, (NodeKindId, String)> {
    let mut out = HashMap::new();
    for g in &r.merged.graphs {
        for (id, kind) in &g.nav.kind_by_id {
            let q = g.nav.qname_by_id.get(id).cloned().unwrap_or_default();
            out.entry(*id).or_insert((*kind, q));
        }
    }
    out
}

/// One edge read back: `(from qname, to qname, from kind, to kind, base
/// rule)`, kinds as their raw ids so a row sorts.
type Row = (String, String, u32, u32, Option<String>);

/// Every edge of `category` (repo and cross edges), sorted.
fn edges_of(r: &GenerateResult, category: EdgeCategoryId) -> Vec<Row> {
    let nav = nav(r);
    let at = |id: NodeId| nav.get(&id).cloned().unwrap_or((NodeKindId(u32::MAX), String::new()));
    let mut out: Vec<_> = r
        .merged
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| {
            let (fk, fq) = at(e.from);
            let (tk, tq) = at(e.to);
            let rule = Evidence::of(e)
                .and_then(|ev| ev.rule)
                .map(|r| same_as::split_evidence_rule(&r).0.to_string());
            (fq, tq, fk.0, tk.0, rule)
        })
        .collect();
    out.sort();
    out
}

fn with_rule(rows: &[Row], rule: &str) -> usize {
    rows.iter().filter(|r| r.4.as_deref() == Some(rule)).count()
}

#[test]
fn no_shares_dependency() {
    let r = build("identity-npm-package", &["web", "admin"]);
    assert!(edges_of(&r, edge_category::SHARES_DEPENDENCY).is_empty());
    let same = edges_of(&r, edge_category::SAME_AS);
    assert_eq!(with_rule(&same, same_as::RULE_PACKAGE_NAME), 2, "{same:?}");

    let r = build("identity-npm-workspace-lib", &["monorepo", "admin"]);
    assert!(edges_of(&r, edge_category::SHARES_DEPENDENCY).is_empty());
    let same = edges_of(&r, edge_category::SAME_AS);
    assert_eq!(with_rule(&same, same_as::RULE_PACKAGE_NAME), 1, "{same:?}");
}

#[test]
fn no_message_shares_schema() {
    let r = build("identity-proto-diverged", &["orders", "billing", "shipping"]);
    let message = node_kind::MESSAGE_TYPE.0;
    let messages = |rows: &[Row]| rows.iter().filter(|r| r.2 == message && r.3 == message).count();
    let shares = edges_of(&r, edge_category::SHARES_SCHEMA);
    assert_eq!(messages(&shares), 0, "{shares:?}");
    let same = edges_of(&r, edge_category::SAME_AS);
    assert_eq!(messages(&same), 2, "a star of three copies: {same:?}");
    assert_eq!(with_rule(&same, same_as::RULE_MESSAGE_FQNAME), 2, "{same:?}");

    // The two-copy message fixtures whose key.json forbids the pair: one star
    // edge per shared message (xschema-jsonschema-shared shares OrderCreated
    // and its $defs entry OrderCreated.Address), no SHARES_SCHEMA.
    for (name, ds, shared) in [
        ("xcut-proto-shared", ["svc-a", "svc-b"], 1),
        ("xschema-jsonschema-shared", ["orders", "billing"], 2),
        ("proto-field-drift", ["producer", "consumer"], 1),
    ] {
        let r = build(name, &ds);
        let shares = edges_of(&r, edge_category::SHARES_SCHEMA);
        assert_eq!(messages(&shares), 0, "{name}: {shares:?}");
        let same = edges_of(&r, edge_category::SAME_AS);
        assert_eq!(messages(&same), shared, "{name}: {same:?}");
        assert_eq!(with_rule(&same, same_as::RULE_MESSAGE_FQNAME), shared, "{name}: {same:?}");
        assert!(same.iter().all(|r| r.0 == r.1), "{name}: equal qnames: {same:?}");
    }

    // SharedSchemaResolver's DTO pair is sharing, not identity: it stays.
    let r = build("xschema-csharp-namespace", &["svc_a", "svc_b"]);
    let shares = edges_of(&r, edge_category::SHARES_SCHEMA);
    assert_eq!(
        shares
            .iter()
            .map(|r| (r.0.as_str(), r.1.as_str(), r.2))
            .collect::<Vec<_>>(),
        [(
            "Shop::Contracts::OrderDto",
            "Store::Contracts::OrderDto",
            node_kind::CLASS.0
        )],
        "{shares:?}"
    );
}

/// `schema_copy` pairs every cross-repo member pair of the class, as the
/// clique did (three copies, three rows), not only the two star edges.
#[test]
fn schema_copy_pairs_the_whole_class() {
    let r = build("identity-proto-diverged", &["orders", "billing", "shipping"]);
    let rows = contract_fields(&r.merged);
    let copies: Vec<_> = rows.iter().filter(|r| r.pairing == "schema_copy").collect();
    assert_eq!(copies.len(), 3, "{rows:#?}");
    for c in &copies {
        assert_eq!(c.key, "shop.v1.OrderCreated");
        assert_ne!(c.producer.repo_id, c.consumer.repo_id, "{c:#?}");
    }
}

#[test]
fn live_set_unchanged() {
    for (name, ds) in [
        ("identity-proto-diverged", &["orders", "billing", "shipping"][..]),
        ("proto-field-drift", &["producer", "consumer"][..]),
    ] {
        let r = build(name, ds);
        let nav = nav(&r);
        let messages = nav
            .values()
            .filter(|(k, _)| *k == node_kind::MESSAGE_TYPE)
            .count();
        assert!(messages >= 2, "{name}: {messages} MESSAGE_TYPEs");
        let live = entrypoint_reachable(&r.merged);
        let live_messages: Vec<&String> = live
            .iter()
            .filter_map(|id| nav.get(id))
            .filter(|(k, _)| *k == node_kind::MESSAGE_TYPE)
            .map(|(_, q)| q)
            .collect();
        assert!(live_messages.is_empty(), "{name}: {live_messages:?}");
    }
}

const CHILD_ENV: &str = "GLIA_DD16_CHILD_DIRS";

/// Builds the `|`-separated dirs named by [`CHILD_ENV`]; a no-op in a normal
/// run.
#[test]
fn child_build_for_stderr() {
    let Ok(dirs) = std::env::var(CHILD_ENV) else {
        return;
    };
    let paths: Vec<String> = dirs.split('|').map(String::from).collect();
    generate_many(&paths).unwrap();
}

/// fired_on: the `[identity]` line carries the message class alone, and the
/// retired resolver's `[schema-link]` line is gone.
#[test]
fn marker() {
    let out = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", "child_build_for_stderr", "--nocapture", "--test-threads=1"])
        .env(
            CHILD_ENV,
            dirs("identity-proto-diverged", &["orders", "billing", "shipping"]).join("|"),
        )
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child build failed:\n{stderr}");
    assert!(
        !stderr.lines().any(|l| l.starts_with("[schema-link]")),
        "{stderr}"
    );
    let identity: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[identity] classes="))
        .collect();
    assert_eq!(identity.len(), 1, "{stderr}");
    assert!(
        identity[0].contains(" message_fqname=1 ") && identity[0].contains(" edges=2 "),
        "{}",
        identity[0]
    );
}
