//! LA.1a: Rust path calls resolve through the `extra_hook` seam of
//! `build_rust` (graph/src/rust_paths.rs), fed with the Cargo packages the
//! walk found. Before LA.1a Rust built with `build_dotted` and a no-op hook,
//! so every `crate::` / `self::` / `super::` / `Self::` / `other_crate::` call
//! was dropped. Run with `-- --nocapture` to see the `[rust-paths]` marker.

use std::collections::HashMap;
use std::path::Path;

use repo_graph_code_domain::edge_category;
use repo_graph_core::NodeId;
use repo_graph_engine::{GenerateResult, entrypoint_reachable, generate_one};

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/rust-crate-path-calls"
    )
    .to_string()
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, text).expect("write");
}

/// Every CALLS edge of the build as `(from qname, to qname)`, sorted, with
/// duplicates kept (one edge per call site).
fn calls(r: &GenerateResult) -> Vec<(String, String)> {
    let qname = qnames(r);
    let mut out: Vec<(String, String)> = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::CALLS)
        .map(|e| {
            let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
            (name(&e.from), name(&e.to))
        })
        .collect();
    out.sort();
    out
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

fn count(calls: &[(String, String)], from: &str, to: &str) -> usize {
    calls.iter().filter(|(f, t)| f == from && t == to).count()
}

#[test]
fn crate_paths_resolve() {
    let r = generate_one(&fixture()).expect("fixture builds");
    let calls = calls(&r);
    for (from, to, form) in [
        (
            "app::src::main::main",
            "core::src::api::service_map",
            "acme_core::service_map()",
        ),
        (
            "app::src::main::main",
            "core::src::api::helper",
            "acme_core::api::helper()",
        ),
        (
            "app::src::main::main",
            "core::src::api::Engine::new",
            "acme_core::api::Engine::new()",
        ),
        (
            "core::src::api::helper",
            "core::src::lib::root_fn2",
            "super::root_fn2()",
        ),
        (
            "core::src::api::Engine::run",
            "core::src::api::Engine::step",
            "Self::step()",
        ),
    ] {
        assert_eq!(
            count(&calls, from, to),
            1,
            "{form}: {from} -> {to}; CALLS = {calls:?}"
        );
    }
    assert_eq!(
        count(&calls, "core::src::lib::root_fn", "core::src::api::helper"),
        2,
        "crate::api::helper() and self::api::helper() are one edge each; CALLS = {calls:?}"
    );
    assert_eq!(
        count(&calls, "app::src::main::main", "app::src::main::helper"),
        0,
        "acme_core::api::helper() must never bind the caller's own helper; CALLS = {calls:?}"
    );
    // The bare control resolved before LA.1a and still does.
    assert_eq!(
        count(
            &calls,
            "core::src::api::service_map",
            "core::src::api::helper"
        ),
        1
    );

    let live = entrypoint_reachable(&r.merged);
    let q = qnames(&r);
    let live_q: Vec<&str> = live
        .iter()
        .filter_map(|id| q.get(id).map(String::as_str))
        .collect();
    for target in ["core::src::api::service_map", "core::src::api::Engine::new"] {
        assert!(
            live_q.contains(&target),
            "{target} is live from app's main; live = {live_q:?}"
        );
    }
}

/// Two workspaces whose packages share names: each app's `acme_core::` binds
/// into its OWN sibling core, and a caller equally near to both binds neither.
#[test]
fn duplicate_crate_names_bind_to_the_nearest_workspace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    for ws in ["ws1", "ws2"] {
        write(
            root,
            &format!("{ws}/Cargo.toml"),
            "[workspace]\nmembers = [\"core\", \"app\"]\n",
        );
        write(
            root,
            &format!("{ws}/core/Cargo.toml"),
            "[package]\nname = \"acme-core\"\n",
        );
        write(root, &format!("{ws}/core/src/lib.rs"), "pub mod api;\n");
        write(
            root,
            &format!("{ws}/core/src/api.rs"),
            "pub fn helper() -> u32 {\n    1\n}\n",
        );
        write(
            root,
            &format!("{ws}/app/Cargo.toml"),
            "[package]\nname = \"acme-app\"\n",
        );
        write(
            root,
            &format!("{ws}/app/src/main.rs"),
            "fn main() {\n    acme_core::api::helper();\n}\n",
        );
    }
    write(root, "tool/Cargo.toml", "[package]\nname = \"tool\"\n");
    write(
        root,
        "tool/src/main.rs",
        "fn main() {\n    acme_core::api::helper();\n}\n",
    );

    let r = generate_one(root.to_str().expect("utf-8 path")).expect("builds");
    let calls = calls(&r);
    for (a, b) in [("ws1", "ws2"), ("ws2", "ws1")] {
        let from = format!("{a}::app::src::main::main");
        assert_eq!(
            count(&calls, &from, &format!("{a}::core::src::api::helper")),
            1,
            "{calls:?}"
        );
        assert_eq!(
            count(&calls, &from, &format!("{b}::core::src::api::helper")),
            0,
            "{calls:?}"
        );
    }
    assert!(
        !calls.iter().any(|(f, _)| f == "tool::src::main::main"),
        "equally near to ws1 and ws2 is a tie: unresolved, never a guess; CALLS = {calls:?}"
    );
}

/// Loose `.rs` files with no Cargo.toml: the Bare call through the `use`
/// binds as before LA.1a, and `crate::` takes the repo root as the crate dir.
#[test]
fn loose_files_keep_head_behaviour() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "main.rs",
        "mod util;\nuse crate::util::greet;\n\nfn main() {\n    greet();\n    crate::util::greet();\n}\n",
    );
    write(root, "util.rs", "pub fn greet() {}\n");
    let r = generate_one(root.to_str().expect("utf-8 path")).expect("builds");
    let calls = calls(&r);
    assert_eq!(
        count(&calls, "main::main", "util::greet"),
        2,
        "CALLS = {calls:?}"
    );
}

/// `Self::` in an `impl` written in another file than its type, a crate path
/// to a method of that impl, `super::super::`, and the lone-base guard: a
/// local `model` value's `model.pow(2)` is a method call, never a CALLS edge
/// into the same-named child module's `pow`.
#[test]
fn impl_elsewhere_nested_super_and_value_receivers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"shop\"\n");
    write(
        root,
        "src/lib.rs",
        "pub mod model;\npub mod ops;\n\npub fn boot() {\n    crate::model::Engine::start();\n}\n\n\
         pub fn value() -> u32 {\n    let model = 3u32;\n    model.pow(2)\n}\n",
    );
    write(
        root,
        "src/model.rs",
        "pub struct Engine;\n\npub fn make() {}\n\npub fn pow() {}\n",
    );
    write(
        root,
        "src/ops.rs",
        "pub mod deep;\nuse crate::model::Engine;\n\nimpl Engine {\n    pub fn start() {\n        Self::warm();\n    }\n    fn warm() {}\n}\n",
    );
    write(
        root,
        "src/ops/deep.rs",
        "pub fn f() {\n    super::super::model::make();\n}\n",
    );

    let r = generate_one(root.to_str().expect("utf-8 path")).expect("builds");
    let calls = calls(&r);
    assert_eq!(
        count(&calls, "src::lib::boot", "src::ops::Engine::start"),
        1,
        "{calls:?}"
    );
    assert_eq!(
        count(&calls, "src::ops::Engine::start", "src::ops::Engine::warm"),
        1,
        "{calls:?}"
    );
    assert_eq!(
        count(&calls, "src::ops::deep::f", "src::model::make"),
        1,
        "{calls:?}"
    );
    assert_eq!(
        count(&calls, "src::lib::value", "src::model::pow"),
        0,
        "{calls:?}"
    );
}
