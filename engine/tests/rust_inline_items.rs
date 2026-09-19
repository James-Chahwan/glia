//! LA.3: Rust inline `mod x { .. }` blocks are PACKAGE nodes holding every
//! item inside them, enum variants are ATTRIBUTE nodes that references USE,
//! and a `self.m()` in an `impl` written outside its type's file binds.
//!
//! The first test builds bench/substrate-gap/fixtures/rust-inline-mod-enum
//! (its `src/lib.rs`, verbatim) in a temp dir and asserts every
//! `expect_edges` row and all three `forbid` rows by qname. The shadowing
//! forbid is the one that fails when an inline-mod Bare call goes through the
//! generic order (file module before PACKAGE) instead of `build_rust`'s
//! innermost-first pre-pass. Run with `-- --nocapture` to see the
//! `[rust-items]` marker.

use std::collections::HashMap;
use std::path::Path;

use glia_code_domain::{edge_category, node_kind};
use glia_core::{EdgeCategoryId, NodeId};
use glia_engine::{GenerateResult, generate_one};

const FIXTURE_LIB: &str =
    include_str!("../../bench/substrate-gap/fixtures/rust-inline-mod-enum/src/lib.rs");

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, text).expect("write");
}

fn build(files: &[(&str, &str)]) -> GenerateResult {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, text) in files {
        write(dir.path(), rel, text);
    }
    generate_one(dir.path().to_str().expect("utf-8 path")).expect("builds")
}

fn qnames(r: &GenerateResult) -> HashMap<NodeId, String> {
    let mut qname = HashMap::new();
    for g in &r.merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
    }
    qname
}

fn kind_of(r: &GenerateResult, qname: &str) -> Vec<glia_core::NodeKindId> {
    let mut kinds: Vec<_> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .filter(|(_, q)| q.as_str() == qname)
                .filter_map(|(id, _)| g.nav.kind_by_id.get(id).copied())
        })
        .collect();
    kinds.sort_by_key(|k| k.0);
    kinds
}

/// Every edge of `category` as `(from qname, to qname)`, sorted, duplicates kept.
fn edges(r: &GenerateResult, category: EdgeCategoryId) -> Vec<(String, String)> {
    let qname = qnames(r);
    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let mut out: Vec<(String, String)> = r
        .merged
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (name(&e.from), name(&e.to)))
        .collect();
    out.sort();
    out
}

fn has(list: &[(String, String)], from: &str, to: &str) -> bool {
    list.iter().any(|(f, t)| f == from && t == to)
}

#[test]
fn fixture_rows_hold() {
    let r = build(&[("src/lib.rs", FIXTURE_LIB)]);

    for (q, kind) in [
        ("src::lib::endpoint", node_kind::PACKAGE),
        ("src::lib::endpoint::inner", node_kind::PACKAGE),
        ("src::lib::tests", node_kind::PACKAGE),
        ("src::lib::endpoint::url_to_path", node_kind::FUNCTION),
        ("src::lib::endpoint::helper", node_kind::FUNCTION),
        ("src::lib::endpoint::inner::deep", node_kind::FUNCTION),
        ("src::lib::tests::tier_is_base_fold", node_kind::FUNCTION),
        ("src::lib::MatchTier::Exact", node_kind::ATTRIBUTE),
        ("src::lib::MatchTier::BaseFold", node_kind::ATTRIBUTE),
        ("src::lib::MatchTier::Suffix", node_kind::ATTRIBUTE),
        ("src::lib::MatchTier::Named", node_kind::ATTRIBUTE),
    ] {
        assert_eq!(kind_of(&r, q), vec![kind], "{q}");
    }

    let contains = edges(&r, edge_category::CONTAINS);
    assert!(
        has(&contains, "src::lib", "src::lib::endpoint"),
        "{contains:?}"
    );
    assert!(has(
        &contains,
        "src::lib::endpoint",
        "src::lib::endpoint::inner"
    ));
    assert!(has(&contains, "src::lib", "src::lib::tests"));

    let has_attr = edges(&r, edge_category::HAS_ATTRIBUTE);
    for v in ["Exact", "BaseFold", "Suffix", "Named"] {
        let to = format!("src::lib::MatchTier::{v}");
        assert!(
            has(&has_attr, "src::lib::MatchTier", &to),
            "{v}: {has_attr:?}"
        );
    }

    let uses = edges(&r, edge_category::USES);
    assert_eq!(
        uses,
        vec![
            (
                "src::lib::classify".into(),
                "src::lib::MatchTier::Exact".into()
            ),
            (
                "src::lib::classify".into(),
                "src::lib::MatchTier::Suffix".into()
            ),
            ("src::lib::mk".into(), "src::lib::MatchTier::Suffix".into()),
            (
                "src::lib::named".into(),
                "src::lib::MatchTier::Named".into()
            ),
            (
                "src::lib::tier".into(),
                "src::lib::MatchTier::BaseFold".into()
            ),
        ],
        "one USES per variant named, and only those"
    );

    let calls = edges(&r, edge_category::CALLS);
    for (from, to) in [
        ("src::lib::use_ep", "src::lib::endpoint::url_to_path"),
        ("src::lib::use_inner", "src::lib::endpoint::inner::deep"),
        (
            "src::lib::endpoint::url_to_path",
            "src::lib::endpoint::helper",
        ),
        ("src::lib::tests::tier_is_base_fold", "src::lib::tier"),
        ("src::lib::MatchTier::rank", "src::lib::MatchTier::weight"),
    ] {
        assert!(has(&calls, from, to), "{from} -> {to}: {calls:?}");
    }

    // forbid rows.
    assert!(!has(&uses, "src::lib::tier", "src::lib::MatchTier::Exact"));
    assert!(
        kind_of(&r, "src::lib::url_to_path").is_empty(),
        "no flattened fn"
    );
    assert!(
        !has(
            &calls,
            "src::lib::endpoint::url_to_path",
            "src::lib::helper"
        ),
        "the inline mod's own `helper` shadows the file-level one: {calls:?}"
    );
    // `MatchTier::Suffix(3)` constructs a variant: never a CALLS edge.
    assert!(!calls.iter().any(|(f, _)| f == "src::lib::mk"), "{calls:?}");
}

/// A test mod reaches its parent's items the way `use super::*` does: bare
/// calls through the generic order, paths and variants through the enclosing
/// scopes. Nested mods bind innermost first, and an inner mod's `super::`
/// is its nav parent.
#[test]
fn test_mod_reaches_parent_scope() {
    let lib = r#"
pub enum Kind { A, B }
pub fn top() -> Kind { Kind::A }
pub mod util {
    pub fn go() -> u8 { 1 }
    pub fn pick() -> u8 { 2 }
    pub mod deep {
        pub fn pick() -> u8 { 3 }
        pub fn run() -> u8 { pick() + super::go() }
    }
    pub fn run() -> u8 { pick() }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn it() {
        let _ = top();
        let _ = util::go();
        assert!(matches!(Kind::B, Kind::B));
        let _ = Kind::B;
    }
}
"#;
    let r = build(&[("src/lib.rs", lib)]);
    let calls = edges(&r, edge_category::CALLS);
    for (from, to) in [
        ("src::lib::tests::it", "src::lib::top"),
        ("src::lib::tests::it", "src::lib::util::go"),
        ("src::lib::util::deep::run", "src::lib::util::deep::pick"),
        ("src::lib::util::deep::run", "src::lib::util::go"),
        ("src::lib::util::run", "src::lib::util::pick"),
    ] {
        assert!(has(&calls, from, to), "{from} -> {to}: {calls:?}");
    }
    assert!(!has(
        &calls,
        "src::lib::util::deep::run",
        "src::lib::util::pick"
    ));
    assert!(!has(
        &calls,
        "src::lib::util::run",
        "src::lib::util::deep::pick"
    ));
    let uses = edges(&r, edge_category::USES);
    assert!(
        has(&uses, "src::lib::tests::it", "src::lib::Kind::B"),
        "{uses:?}"
    );
    assert!(has(&uses, "src::lib::top", "src::lib::Kind::A"), "{uses:?}");
}

/// `impl Graph` written in another file than `struct Graph`: the methods
/// parent to their MODULE, and `self.m()` / `Self::m()` bind through the one
/// crate-local `Graph`, its own-file impl first, then the other-file impls.
/// The reverse direction too: a method of the enum's own-file impl reaching
/// a member written in another file's `impl Kind`. `Self::A` /
/// `crate::model::Kind::B` bind the variant.
#[test]
fn impl_elsewhere_self_calls_and_variant_paths() {
    let lib = "pub mod model;\npub mod blast;\n";
    let model = r#"
pub struct Graph;
impl Graph {
    pub fn all(&self) -> u8 { 1 }
}
pub enum Kind { A, B }
impl Kind {
    pub fn first() -> Self { Self::A }
    pub fn is_first(&self) -> bool { self.rank() == 0 }
}
"#;
    let blast = r#"
use crate::model::{Graph, Kind};
impl Graph {
    pub fn blast(&self) -> u8 { self.all() + self.near() + Self::near(self) }
    fn near(&self) -> u8 { 2 }
}
impl Kind {
    pub fn rank(&self) -> u8 { 0 }
}
pub fn second() -> crate::model::Kind { crate::model::Kind::B }
"#;
    let r = build(&[
        ("src/lib.rs", lib),
        ("src/model.rs", model),
        ("src/blast.rs", blast),
    ]);
    let calls = edges(&r, edge_category::CALLS);
    for (from, to) in [
        ("src::blast::Graph::blast", "src::model::Graph::all"),
        ("src::blast::Graph::blast", "src::blast::Graph::near"),
        ("src::model::Kind::is_first", "src::blast::Kind::rank"),
    ] {
        assert!(has(&calls, from, to), "{from} -> {to}: {calls:?}");
    }
    let uses = edges(&r, edge_category::USES);
    assert!(
        has(&uses, "src::model::Kind::first", "src::model::Kind::A"),
        "{uses:?}"
    );
    assert!(
        has(&uses, "src::blast::second", "src::model::Kind::B"),
        "{uses:?}"
    );
}
