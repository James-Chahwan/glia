//! LC.4 — `glia inspect` names a layout's ids from the files themselves.
//!
//! Builds `tests/fixtures/py_smoke` in a scratch copy, writes its layout with
//! `write_merged_sharded`, and drives the real binary. The fired_on marker is
//! grep-able:
//! `cargo test -p glia-cli --test inspect_cli -- --nocapture 2>&1 | grep -o '\[inspect\] .*'`

use std::path::{Path, PathBuf};
use std::process::Command;

/// A scratch dir, created fresh and removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`).
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-lc4-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).expect("copy fixture file");
        }
    }
}

fn glia(args: &[&str]) -> (i32, serde_json::Value, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("run glia");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    for line in stderr.lines().filter(|l| l.starts_with("[inspect] ")) {
        eprintln!("{line}");
    }
    let json = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    (out.status.code().unwrap_or(-1), json, stderr)
}

fn count_of(kinds: &serde_json::Value, name: &str) -> u64 {
    kinds
        .as_array()
        .expect("kinds is an array")
        .iter()
        .find(|k| k["name"] == name)
        .and_then(|k| k["count"].as_u64())
        .unwrap_or(0)
}

#[test]
fn inspect_names_kinds_from_the_file() {
    let s = Scratch::new("layout");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/py_smoke");
    let repo = s.0.join("repo");
    copy_tree(&fixture, &repo);
    let layout = s.0.join("layout");
    let built =
        glia_engine::generate_one(repo.to_str().expect("utf-8 path")).expect("generate py_smoke");
    let manifest = glia_store::write_merged_sharded(&built.merged, &layout).expect("write layout");
    let layout_arg = layout.to_str().expect("utf-8 path");

    let (code, json, stderr) = glia(&["inspect", layout_arg, "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(json["manifest_schema"], glia_store::MANIFEST_VERSION);
    let shards = json["shards"].as_array().expect("shards");
    assert_eq!(
        shards.len(),
        manifest.shards.len() + manifest.cross.iter().count()
    );
    assert!(shards.iter().all(|s| s["graph_type"] == "code"), "{json}");
    let kinds = &json["totals"]["kinds"];
    assert!(count_of(kinds, "MODULE") > 0, "no MODULE in {kinds}");
    assert!(count_of(kinds, "FUNCTION") > 0, "no FUNCTION in {kinds}");
    assert_eq!(json["totals"]["unregistered"], 0, "{json}");
    // HT.2: every row names its id block; a code layout's are all `code`.
    for table in ["kinds", "categories", "node_cells", "edge_cells"] {
        let rows = json["totals"][table].as_array().expect("rows");
        assert!(
            rows.iter().all(|r| r["block"] == "code"),
            "{table}: {rows:?}"
        );
    }
    assert_eq!(
        json["totals"]["nodes"].as_u64(),
        Some(
            built
                .merged
                .graphs
                .iter()
                .map(|g| g.nodes.len() as u64)
                .sum::<u64>()
        )
    );

    let marker = stderr
        .lines()
        .find(|l| l.starts_with(&format!("[inspect] {layout_arg}: shards={} ", shards.len())))
        .unwrap_or_else(|| panic!("no [inspect] marker in stderr: {stderr}"));
    assert!(marker.contains(" graph_types=code "), "{marker}");
    assert!(marker.ends_with(" unregistered=0"), "{marker}");

    // One shard file on its own is inspectable too, and the table form names
    // the same kinds.
    let shard_file = layout.join(&manifest.shards[0].path);
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["inspect", shard_file.to_str().expect("utf-8 path")])
        .output()
        .expect("run glia");
    assert_eq!(out.status.code(), Some(0));
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("| MODULE |"), "{table}");
    assert!(table.contains("single file"), "{table}");
    // HT.2: a code layout's ids are all below 0x10000, so its id cells stay
    // decimal (the pre-HT.2 text, byte for byte): no hex, no block label.
    assert!(table.contains("| 1 | MODULE |"), "{table}");
    assert!(!table.contains("| 0x"), "{table}");
}

/// HT.2: a file whose ids sit outside the code block prints each such id as
/// `0x<8 hex> [<block>]` in the tables, and every JSON row carries `block`.
#[test]
fn inspect_names_the_block_of_an_id_outside_code() {
    use glia_core::ids::{BUILD, id_in};
    use glia_core::{Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId};

    let s = Scratch::new("blocks");
    let target = id_in(BUILD, 1);
    let depends = id_in(BUILD, 1);
    let header = glia_store::Header::for_domain(
        "build-targets",
        &[(target, "TARGET"), (1, "MODULE")],
        &[(depends, "DEPENDS_ON")],
        &[],
    )
    .expect("header");
    let repo = RepoId::from_canonical("test://ht2-blocks");
    let node = |id: u64| Node {
        id: NodeId(id),
        repo,
        confidence: Confidence::Strong,
        cells: vec![],
    };
    let mut core = glia_store::Container {
        header,
        repo,
        nodes: vec![node(10), node(20), node(30)],
        edges: vec![Edge {
            from: NodeId(10),
            to: NodeId(20),
            category: EdgeCategoryId(depends),
            confidence: Confidence::Strong,
            cells: vec![],
        }],
        node_kinds: vec![
            (NodeId(10), NodeKindId(target)),
            (NodeId(20), NodeKindId(target)),
            (NodeId(30), NodeKindId(1)),
        ],
        sections: Vec::new(),
    };
    let path = s.0.join("targets.gmap");
    glia_store::write_container(&path, &mut core, &[]).expect("write");
    let arg = path.to_str().expect("utf-8 path");

    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["inspect", arg])
        .output()
        .expect("run glia");
    assert_eq!(out.status.code(), Some(0));
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(
        table.contains("| 0x00020001 [build] | TARGET | 2 |"),
        "{table}"
    );
    assert!(
        table.contains("| 0x00020001 [build] | DEPENDS_ON | 1 |"),
        "{table}"
    );
    assert!(table.contains("| 1 | MODULE | 1 |"), "{table}");

    let (code, json, stderr) = glia(&["inspect", arg, "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let kinds = json["totals"]["kinds"].as_array().expect("kinds");
    let rows: Vec<(u64, &str, &str)> = kinds
        .iter()
        .map(|k| {
            (
                k["id"].as_u64().unwrap_or_default(),
                k["name"].as_str().unwrap_or_default(),
                k["block"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (1, "MODULE", "code"),
            (u64::from(target), "TARGET", "build")
        ]
    );
    assert_eq!(json["totals"]["categories"][0]["block"], "build", "{json}");
}

#[test]
fn inspect_fails_with_the_rebuild_reason() {
    let s = Scratch::new("old");
    let old = s.0.join("old.gmap");
    std::fs::write(&old, b"no preamble: a 0.4.x file").expect("write");
    let (code, _, stderr) = glia(&["inspect", old.to_str().expect("utf-8 path")]);
    assert_eq!(code, 1, "stderr: {stderr}");
    assert!(stderr.contains("rebuild the graph"), "{stderr}");
    assert!(
        !stderr.contains("[inspect] "),
        "no marker on failure: {stderr}"
    );
}
