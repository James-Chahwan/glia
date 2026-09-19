//! LB.6 gate on real builds: qnames are path-derived, so a moved file is a
//! delete + add to anything that compares qnames. `detect_moves` pairs the
//! files and aligns their declarations instead; an unrelated file that only
//! shares a basename is rejected; a stored identity hint rebinds to the moved
//! node; a declared (VCS) rename is reported as a FACT.
//!
//! v1: `api/orders.py`, `svc/users.py`.
//! v2: `svc/users.py` moved byte-identical to `services/users/users.py`,
//!     `api/orders.py` moved to `api/v2/orders.py` with `refund_order` added.
//! v3: v1 with `svc/users.py` deleted and an unrelated `lib/users.py` added.
//!
//! Every version is built from a directory named `proj`, so LB.1 derives one
//! RepoId for all three and unmoved nodes keep their NodeIds.

use std::path::Path;

use glia_code_domain::node_kind;
use glia_core::NodeId;
use glia_engine::{GenerateResult, generate_one};
use glia_graph::identity::{
    FileMove, IdentityIndex, MoveTier, Rebind, detect_moves, detect_moves_with, identity_of,
};

const ORDERS_V1: &str = "def list_orders():\n    return []\n\ndef get_order(oid):\n    return oid\n\ndef cancel_order(oid):\n    return None\n";
const REFUND: &str = "\ndef refund_order(oid):\n    return oid\n";
const USERS: &str = "class User:\n    def get(self, uid):\n        return uid\n\ndef load_users():\n    return [User()]\n";
const PING: &str = "def ping():\n    return 1\n";

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(p, body).expect("write");
}

fn build(root: &Path) -> GenerateResult {
    generate_one(root.to_str().expect("utf-8 tempdir")).expect("build")
}

/// The three versions, each under `<tmp>/<v>/proj`.
struct Versions {
    _tmp: tempfile::TempDir,
    v1: GenerateResult,
    v2: GenerateResult,
    v3: GenerateResult,
}

fn versions() -> Versions {
    let tmp = tempfile::tempdir().expect("tempdir");
    let v1 = tmp.path().join("v1").join("proj");
    write(&v1, "api/orders.py", ORDERS_V1);
    write(&v1, "svc/users.py", USERS);
    let v2 = tmp.path().join("v2").join("proj");
    write(&v2, "services/users/users.py", USERS);
    write(&v2, "api/v2/orders.py", &format!("{ORDERS_V1}{REFUND}"));
    let v3 = tmp.path().join("v3").join("proj");
    write(&v3, "api/orders.py", ORDERS_V1);
    write(&v3, "lib/users.py", PING);
    Versions {
        v1: build(&v1),
        v2: build(&v2),
        v3: build(&v3),
        _tmp: tmp,
    }
}

fn qid(r: &GenerateResult, qname: &str) -> NodeId {
    r.merged
        .node_id_by_qname(qname)
        .unwrap_or_else(|| panic!("no node {qname}"))
}

#[test]
fn moves_are_detected_not_deleted() {
    let v = versions();
    assert_eq!(
        v.v1.merged
            .graphs
            .iter()
            .map(|g| g.repo)
            .collect::<Vec<_>>(),
        v.v2.merged
            .graphs
            .iter()
            .map(|g| g.repo)
            .collect::<Vec<_>>(),
        "LB.1: one directory name, one RepoId"
    );
    let map = detect_moves(&v.v1.merged, &v.v2.merged);
    assert_eq!(
        map.files,
        vec![
            FileMove {
                old_path: "api/orders.py".into(),
                new_path: "api/v2/orders.py".into(),
                tier: MoveTier::SameName,
            },
            FileMove {
                old_path: "svc/users.py".into(),
                new_path: "services/users/users.py".into(),
                tier: MoveTier::Identical,
            },
        ]
    );
    assert_eq!(map.rejected, 0);
    let v1_nodes: usize = v.v1.merged.graphs.iter().map(|g| g.nodes.len()).sum();
    assert_eq!(v1_nodes, 8);
    assert_eq!(
        map.nodes.len(),
        8,
        "every v1 node, both MODULEs included: {:#?}",
        map.nodes
    );
    assert!(
        !map.nodes
            .iter()
            .any(|n| n.new_qname == "api::v2::orders::refund_order"),
        "refund_order is a real addition"
    );
    let get = map
        .nodes
        .iter()
        .find(|n| n.old_qname == "svc::users::User::get")
        .expect("method aligned");
    assert_eq!(get.new_qname, "services::users::users::User::get");
    assert_eq!(get.kind, node_kind::METHOD);
}

#[test]
fn unrelated_same_name_file_is_not_a_move() {
    let v = versions();
    let map = detect_moves(&v.v1.merged, &v.v3.merged);
    assert!(map.files.is_empty(), "{:#?}", map.files);
    assert!(map.nodes.is_empty());
    assert_eq!(map.rejected, 1);
}

#[test]
fn stored_hint_rebinds_after_a_move() {
    let v = versions();
    let old = qid(&v.v1, "svc::users::User::get");
    let hint = identity_of(&v.v1.merged, old)
        .expect("method identity")
        .hint();
    let idx = IdentityIndex::build(&v.v2.merged);
    assert_eq!(
        idx.rebind(
            "svc::users::User::get",
            Some(node_kind::METHOD),
            Some(&hint)
        ),
        Rebind::Moved {
            id: qid(&v.v2, "services::users::users::User::get"),
            tier: MoveTier::SameName,
        }
    );
    // An unmoved qname still binds exactly.
    let idx3 = IdentityIndex::build(&v.v3.merged);
    assert_eq!(
        idx3.rebind("api::orders::get_order", Some(node_kind::FUNCTION), None),
        Rebind::Exact(qid(&v.v3, "api::orders::get_order"))
    );
}

#[test]
fn declared_rename_is_a_fact() {
    let v = versions();
    let map = detect_moves_with(
        &v.v1.merged,
        &v.v2.merged,
        &[("api/orders.py".into(), "api/v2/orders.py".into())],
    );
    let orders = map
        .files
        .iter()
        .find(|f| f.old_path == "api/orders.py")
        .expect("orders move");
    assert_eq!(orders.new_path, "api/v2/orders.py");
    assert_eq!(orders.tier, MoveTier::Declared);
    assert_eq!(map.nodes.len(), 8);
}

#[test]
fn detection_is_deterministic() {
    let v = versions();
    let a = detect_moves(&v.v1.merged, &v.v2.merged);
    let b = detect_moves(&v.v1.merged, &v.v2.merged);
    assert_eq!(a, b);
    let names: Vec<&str> = a.nodes.iter().map(|n| n.old_qname.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "nodes sorted by old qname");
}
