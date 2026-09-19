//! LA.1a: Rust path calls resolve through the `extra_hook` seam of
//! `build_rust` (graph/src/rust_paths.rs), fed with the Cargo packages the
//! walk found. Before LA.1a Rust built with `build_dotted` and a no-op hook,
//! so every `crate::` / `self::` / `super::` / `Self::` / `other_crate::` call
//! was dropped. Run with `-- --nocapture` to see the `[rust-paths]` marker.
//!
//! LA.1b: `use` trees resolve as Rust paths (`resolve_imports_rust`), so a
//! workspace-crate use, a `pub use` re-export, an alias, a glob and a fn-body
//! use bind, and the IMPORTS cell lists external crates only. Run with
//! `-- --nocapture` to see the `[rust-uses]` marker.

use std::collections::HashMap;
use std::path::Path;

use repo_graph_code_domain::{cell_type, edge_category};
use repo_graph_core::{CellPayload, EdgeCategoryId, NodeId};
use repo_graph_engine::{GenerateResult, entrypoint_reachable, generate_one};

fn fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/rust-crate-path-calls"
    )
    .to_string()
}

fn reexport_fixture() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bench/substrate-gap/fixtures/rust-use-reexport-calls"
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

/// Every edge of `category` as `(from qname, to qname)`, sorted.
fn edges_of(r: &GenerateResult, category: EdgeCategoryId) -> Vec<(String, String)> {
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

/// LA.1b, the rust-use-reexport-calls fixture: `use reexp_lib::{generate_one,
/// Core}` through the crate root's `pub use engine::{.., Engine as Core}`, a
/// fn-body `use .. as gm`, a `use super::*` glob, and the precision guard that
/// `gm` never binds outside the fn that imports it.
#[test]
fn use_reexport_alias_glob_and_fn_scope() {
    let r = generate_one(&reexport_fixture()).expect("fixture builds");
    let calls = calls(&r);
    for (from, to, form) in [
        (
            "app::src::main::main",
            "lib::src::engine::generate_one",
            "use reexp_lib::generate_one -> pub use engine::generate_one",
        ),
        (
            "app::src::main::main",
            "lib::src::engine::Engine::new",
            "use reexp_lib::Core -> pub use engine::Engine as Core",
        ),
        (
            "app::src::main::helpers",
            "lib::src::engine::generate_many",
            "fn-body use reexp_lib::generate_many as gm",
        ),
        (
            "app::src::extra::again",
            "app::src::main::helpers",
            "use super::*",
        ),
    ] {
        assert_eq!(
            count(&calls, from, to),
            1,
            "{form}: {from} -> {to}; CALLS = {calls:?}"
        );
    }
    assert_eq!(
        count(
            &calls,
            "app::src::main::main",
            "lib::src::engine::generate_many"
        ),
        0,
        "gm is bound in helpers() only; CALLS = {calls:?}"
    );
    let imports = edges_of(&r, edge_category::IMPORTS);
    for (from, to) in [
        ("app::src::main", "lib::src::lib"),
        ("app::src::extra", "app::src::main"),
        ("lib::src::lib", "lib::src::engine"),
    ] {
        assert!(
            imports.iter().any(|(f, t)| f == from && t == to),
            "{from} -> {to}; IMPORTS = {imports:?}"
        );
    }
    assert_eq!(
        imports.len(),
        3,
        "one IMPORTS per (file, module): {imports:?}"
    );
}

/// The IMPORTS cell of every node of `module`: exactly one per node, all
/// equal, returned once.
fn imports_cell(r: &GenerateResult, module: &str) -> String {
    let mut payloads = Vec::new();
    for g in &r.merged.graphs {
        for n in &g.nodes {
            let Some(q) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            if q != module && !q.starts_with(&format!("{module}::")) {
                continue;
            }
            let cells: Vec<String> = n
                .cells
                .iter()
                .filter(|c| c.kind == cell_type::IMPORTS)
                .map(|c| match &c.payload {
                    CellPayload::Json(s) | CellPayload::Text(s) => s.clone(),
                    CellPayload::Bytes(_) => String::from("<bytes>"),
                })
                .collect();
            assert_eq!(cells.len(), 1, "{q}: {cells:?}");
            payloads.push(cells[0].clone());
        }
    }
    payloads.sort();
    payloads.dedup();
    assert_eq!(payloads.len(), 1, "{module}: {payloads:?}");
    payloads.remove(0)
}

/// LA.1b: `use serde::Serialize` reaches the IMPORTS cell; the sibling
/// workspace crate (`use reexp_lib::..`) and the child module of a `pub use
/// engine::..` do not.
#[test]
fn imports_cell_lists_external_crates_only() {
    let r = generate_one(&reexport_fixture()).expect("fixture builds");
    assert_eq!(imports_cell(&r, "app::src::main"), r#"["serde"]"#);
    assert_eq!(
        imports_cell(&r, "app::src::extra"),
        "[]",
        "use super::* is local"
    );
    assert_eq!(
        imports_cell(&r, "lib::src::lib"),
        "[]",
        "pub use engine::.. is local"
    );
}

/// A fn-body `use` shadows a file-level item of the same name inside that
/// fn, and only there.
#[test]
fn fn_scoped_use_shadows_file_item() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"lib\", \"app\"]\n",
    );
    write(root, "lib/Cargo.toml", "[package]\nname = \"lib\"\n");
    write(
        root,
        "lib/src/lib.rs",
        "pub fn generate_many() -> u32 {\n    2\n}\n",
    );
    write(root, "app/Cargo.toml", "[package]\nname = \"app\"\n");
    write(
        root,
        "app/src/main.rs",
        "fn gm() -> u32 {\n    0\n}\n\nfn scoped() -> u32 {\n    use lib::generate_many as gm;\n    gm()\n}\n\n\
         fn plain() -> u32 {\n    gm()\n}\n\nfn main() {\n    scoped();\n    plain();\n}\n",
    );
    let r = generate_one(root.to_str().expect("utf-8 path")).expect("builds");
    let calls = calls(&r);
    assert_eq!(
        count(
            &calls,
            "app::src::main::scoped",
            "lib::src::lib::generate_many"
        ),
        1,
        "{calls:?}"
    );
    assert_eq!(
        count(&calls, "app::src::main::scoped", "app::src::main::gm"),
        0,
        "the fn-body use shadows the file-level gm; CALLS = {calls:?}"
    );
    assert_eq!(
        count(&calls, "app::src::main::plain", "app::src::main::gm"),
        1,
        "a sibling fn still binds the file-level gm; CALLS = {calls:?}"
    );
    assert_eq!(
        count(
            &calls,
            "app::src::main::plain",
            "lib::src::lib::generate_many"
        ),
        0,
        "{calls:?}"
    );
}

/// Re-export chains resolve whatever order their files declare them in; a
/// re-export cycle and a name two globs both bring in stay unresolved, never
/// a guess.
#[test]
fn reexport_chains_cycles_and_glob_ambiguity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"shop\"\n");
    write(
        root,
        "src/lib.rs",
        "pub mod a_user;\npub mod b_hub;\npub mod c_mid;\npub mod d_leaf;\n\
         pub mod ping;\npub mod pong;\npub mod one;\npub mod two;\npub mod both;\npub mod solo;\n",
    );
    // a_user -> b_hub -> c_mid -> d_leaf: the use sorts before every re-export.
    write(
        root,
        "src/a_user.rs",
        "use crate::b_hub::deep;\n\npub fn run() -> u32 {\n    deep()\n}\n",
    );
    write(root, "src/b_hub.rs", "pub use crate::c_mid::deep;\n");
    write(root, "src/c_mid.rs", "pub use crate::d_leaf::deep;\n");
    write(root, "src/d_leaf.rs", "pub fn deep() -> u32 {\n    1\n}\n");
    // ping::loopy <-> pong::loopy re-export each other; nothing defines it.
    write(
        root,
        "src/ping.rs",
        "pub use crate::pong::loopy;\n\npub fn f() {\n    loopy();\n}\n",
    );
    write(root, "src/pong.rs", "pub use crate::ping::loopy;\n");
    // `dup` comes in through two globs: ambiguous.
    write(root, "src/one.rs", "pub fn dup() {}\n");
    write(root, "src/two.rs", "pub fn dup() {}\n");
    write(
        root,
        "src/both.rs",
        "use crate::one::*;\nuse crate::two::*;\n\npub fn g() {\n    dup();\n}\n",
    );
    // Control: one glob binds it.
    write(
        root,
        "src/solo.rs",
        "use crate::one::*;\n\npub fn h() {\n    dup();\n}\n",
    );
    let r = generate_one(root.to_str().expect("utf-8 path")).expect("builds");
    let calls = calls(&r);
    assert_eq!(
        count(&calls, "src::a_user::run", "src::d_leaf::deep"),
        1,
        "a three-hop re-export chain; CALLS = {calls:?}"
    );
    assert!(
        !calls.iter().any(|(f, _)| f == "src::ping::f"),
        "a re-export cycle binds nothing; CALLS = {calls:?}"
    );
    assert!(
        !calls.iter().any(|(f, _)| f == "src::both::g"),
        "two globs bringing in `dup` is ambiguous; CALLS = {calls:?}"
    );
    assert_eq!(
        count(&calls, "src::solo::h", "src::one::dup"),
        1,
        "{calls:?}"
    );
}
