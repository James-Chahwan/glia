//! LG.9: `identity_hint` is `<file token>:<kind>:<ordinal>`, and the file
//! token survives a file move along a `--since` chain: each export records
//! the glia graph it exported in `<out>.glia/`, and the next run pairs moved
//! files against it (LB.6 `detect_moves`) and carries their tokens forward.
//! Also: when two nodes share a key, the exported one is the located one.
//!
//! The chain tests run the built bin on scratch dirs (no git: LB.6 pairs by
//! content and by name), with every `--out` outside the exported repo.

use std::path::{Path, PathBuf};
use std::process::Command;

use engram_core::{Content, Gmap, GmapNode, SpanRef};
use glia_code_domain::{CodeNav, cell_type, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};
use glia_engram_export::{ExportOptions, build_gmap, diff::read_gmap, history_dir};
use glia_graph::{MergedGraph, RepoGraph};

/// The LG.7 probe fixture (12, 7 and 18 lines).
const ORDERS_PY: &str = r#""""Shop orders: placing and checking."""

import json


class OrderService:
    def place(self, sku):
        """Place one order."""
        return self.validate(sku)

    def validate(self, sku):
        return bool(json.dumps(sku))
"#;

const README_MD: &str = "# Orders

Orders are placed through the order service.

## Placing orders

Call `OrderService.place` with a sku; payment settles via `Vault.deposit`.
";

const VAULT_SOL: &str = "// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract Vault {
    mapping(address => uint256) public balances;

    /// Deposit ether into the vault
    function deposit(address who) public payable {
        balances[who] += msg.value;
        return;
    }
}
";

/// The four nodes of `orders.py`: key suffixes under its module qname.
const ORDERS_NODES: [&str; 4] = [
    "",
    "::OrderService",
    "::OrderService::place",
    "::OrderService::validate",
];

/// A scratch dir holding the repo (`fx/`) and the exports (`out/`), removed
/// on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-lg9-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("fx")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        let s = Scratch(root);
        s.write("svc/orders.py", ORDERS_PY);
        s.write("docs/README.md", README_MD);
        s.write("contracts/Vault.sol", VAULT_SOL);
        s
    }

    fn repo(&self) -> PathBuf {
        self.0.join("fx")
    }

    fn out(&self, name: &str) -> PathBuf {
        self.0.join("out").join(format!("{name}.engram-gmap"))
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.repo().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn mv(&self, from: &str, to: &str) {
        let to = self.repo().join(to);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::rename(self.repo().join(from), to).unwrap();
    }

    /// Run the bin: export the repo to `out/<name>`, `--since out/<since>`
    /// when given. Returns the gmap and the bin's stderr.
    fn export(&self, name: &str, since: Option<&str>) -> (Gmap, String) {
        let (code, stderr) = self.run(name, since.map(|s| self.out(s)).as_deref());
        assert_eq!(code, Some(0), "export {name} failed:\n{stderr}");
        let (gmap, _) = read_gmap(&self.out(name)).unwrap();
        (gmap, stderr)
    }

    fn run(&self, name: &str, since: Option<&Path>) -> (Option<i32>, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia-export-engram"));
        cmd.arg(self.repo()).arg("--out").arg(self.out(name));
        if let Some(prior) = since {
            cmd.arg("--since").arg(prior);
        }
        let out = cmd.env("GLIA_NO_PERSIST", "1").output().unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn node<'g>(gmap: &'g Gmap, key: &str) -> &'g GmapNode {
    let keys: Vec<&str> = gmap.nodes.iter().map(|n| n.key.as_str()).collect();
    gmap.nodes
        .iter()
        .find(|n| n.key == key)
        .unwrap_or_else(|| panic!("no node {key}; keys: {keys:?}"))
}

fn hint(gmap: &Gmap, key: &str) -> String {
    let n = node(gmap, key);
    n.identity_hint
        .clone()
        .unwrap_or_else(|| panic!("{key} has no identity_hint"))
}

/// The hints of `orders.py`'s four nodes under the module qname `module`.
fn orders_hints(gmap: &Gmap, module: &str) -> Vec<String> {
    ORDERS_NODES
        .iter()
        .map(|s| hint(gmap, &format!("{module}{s}")))
        .collect()
}

/// The `[engram-export] v6 identity: <n>/<m> hints, ...` marker line (the
/// prior-graph warning shares its prefix).
fn identity_line(stderr: &str) -> &str {
    stderr
        .lines()
        .find(|l| l.starts_with("[engram-export] v6 identity: ") && l.contains(" hints, "))
        .unwrap_or_else(|| panic!("no v6 identity marker in:\n{stderr}"))
}

#[test]
fn moved_file_keeps_hints() {
    let s = Scratch::new("moved");
    let (a, a_err) = s.export("a", None);
    assert!(
        identity_line(&a_err).contains(" 0 file(s) under a prior identity (0 move(s) detected)"),
        "{a_err}"
    );
    assert!(
        history_dir(&s.out("a")).join("manifest.json").is_file(),
        "no history for a"
    );
    let before = orders_hints(&a, "svc::orders");
    assert!(
        before.iter().all(|h| h.starts_with("svc/orders.py:")),
        "{before:?}"
    );

    s.mv("svc/orders.py", "svc/sales/orders.py");
    let (b, b_err) = s.export("b", Some("a"));
    assert!(
        identity_line(&b_err).contains(" 1 file(s) under a prior identity (1 move(s) detected)"),
        "{b_err}"
    );
    assert!(
        b.nodes.iter().all(|n| !n.key.starts_with("svc::orders")),
        "old keys remain"
    );
    assert_eq!(orders_hints(&b, "svc::sales::orders"), before);

    // Control: the moved tree exported without --since starts a fresh chain.
    let (fresh, _) = s.export("fresh", None);
    let fresh_hints = orders_hints(&fresh, "svc::sales::orders");
    for (old, new) in before.iter().zip(&fresh_hints) {
        assert_ne!(
            old, new,
            "a fresh export must not carry the moved file's identity"
        );
        assert!(new.starts_with("svc/sales/orders.py:"), "{new}");
    }
}

#[test]
fn rename_and_body_edit_keep_hints() {
    let s = Scratch::new("rename");
    let (a, _) = s.export("a", None);
    let edited = ORDERS_PY
        .replace("def place(self, sku):", "def submit(self, sku):")
        .replace(
            "return self.validate(sku)",
            "return self.validate(sku) and sku",
        );
    s.write("svc/orders.py", &edited);
    let (b, b_err) = s.export("b", Some("a"));
    assert!(
        identity_line(&b_err).contains("(0 move(s) detected)"),
        "{b_err}"
    );
    assert_eq!(
        hint(&b, "svc::orders::OrderService::submit"),
        hint(&a, "svc::orders::OrderService::place")
    );
    for key in [
        "svc::orders",
        "svc::orders::OrderService",
        "svc::orders::OrderService::validate",
    ] {
        assert_eq!(hint(&b, key), hint(&a, key), "{key}");
    }
}

#[test]
fn fixture_validate_is_the_located_method() {
    let s = Scratch::new("validate");
    let (a, a_err) = s.export("a", None);
    let validate = node(&a, "svc::orders::OrderService::validate");
    let Content::Symbol { span, .. } = &validate.content else {
        panic!("validate is not a Symbol: {:?}", validate.content);
    };
    assert_eq!((span.start_line, span.end_line), (11, 12));
    assert_eq!(
        validate.identity_hint.as_deref(),
        Some(format!("svc/orders.py:{}:1", node_kind::METHOD.0).as_str())
    );
    // The ATTRIBUTE twin (the `self.validate` read) is the loser, and counted.
    assert!(identity_line(&a_err).contains("duplicate key(s) resolved to the located node"));
    assert!(
        !identity_line(&a_err).contains(" 0 duplicate key(s)"),
        "{a_err}"
    );
}

#[test]
fn chain_keeps_first_sight_and_suffixes_a_reused_path() {
    let s = Scratch::new("chain");
    let (a, _) = s.export("a", None);
    let first_sight = orders_hints(&a, "svc::orders");
    s.mv("svc/orders.py", "svc/sales/orders.py");
    s.export("b", Some("a"));

    s.mv("svc/sales/orders.py", "svc/core/orders.py");
    s.write("svc/orders.py", "def helper():\n    return 1\n");
    let (c, c_err) = s.export("c", Some("b"));
    assert!(
        identity_line(&c_err).contains(" 1 file(s) under a prior identity (1 move(s) detected)"),
        "{c_err}"
    );
    assert_eq!(orders_hints(&c, "svc::core::orders"), first_sight);
    // A new file where a moved file used to live gets a fresh token (LB.6
    // carry_file_tokens suffixes `#2`), never the moved file's.
    assert_eq!(
        hint(&c, "svc::orders"),
        format!("svc/orders.py#2:{}:0", node_kind::MODULE.0)
    );
    assert_eq!(
        hint(&c, "svc::orders::helper"),
        format!("svc/orders.py#2:{}:0", node_kind::FUNCTION.0)
    );
}

#[test]
fn missing_history_still_carries_tokens_by_path() {
    let s = Scratch::new("nohistory");
    let (a, _) = s.export("a", None);
    s.mv("svc/orders.py", "svc/sales/orders.py");
    s.export("b", Some("a"));
    std::fs::remove_dir_all(history_dir(&s.out("b"))).unwrap();
    // Nothing moves: the files still take their tokens from b's hints.
    let (c, c_err) = s.export("c", Some("b"));
    assert!(
        c_err.contains("[engram-export] v6 identity: prior graph unavailable at "),
        "{c_err}"
    );
    assert!(
        identity_line(&c_err).contains(" 1 file(s) under a prior identity (0 move(s) detected)"),
        "{c_err}"
    );
    assert_eq!(
        orders_hints(&c, "svc::sales::orders"),
        orders_hints(&a, "svc::orders")
    );
}

#[test]
fn since_refuses_an_unreadable_prior() {
    let s = Scratch::new("refused");
    let missing = s.out("never-written");
    let (code, stderr) = s.run("a", Some(&missing));
    assert_eq!(code, Some(6), "{stderr}");
    assert!(
        stderr.contains("[engram-export] v6 since: refused - "),
        "{stderr}"
    );
    assert!(!s.out("a").exists(), "a refused run writes nothing");
}

// ---- duplicate keys, on a hand-built graph ----

fn pos(file: &str, start: u32, end: u32) -> Cell {
    Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(format!(
            r#"{{"file":"{file}","start_line":{start},"end_line":{end}}}"#
        )),
    }
}

fn doc(t: &str) -> Cell {
    Cell {
        kind: cell_type::DOC,
        payload: CellPayload::Text(t.to_string()),
    }
}

/// One graph holding `rows` = `(id, qname, kind, cells)`, in that order.
fn graph(rows: &[(u64, &str, glia_core::NodeKindId, Vec<Cell>)]) -> MergedGraph {
    let repo = RepoId(1);
    let mut nav = CodeNav::default();
    let mut nodes = Vec::new();
    for (id, qname, kind, cells) in rows {
        let name = qname.rsplit("::").next().unwrap();
        nav.record(NodeId(*id), name, qname, *kind, None);
        nodes.push(Node {
            id: NodeId(*id),
            repo,
            confidence: Confidence::Strong,
            cells: cells.clone(),
        });
    }
    MergedGraph::new(vec![RepoGraph {
        repo,
        nodes,
        edges: Vec::new(),
        nav,
        symbols: Default::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    }])
}

fn symbol(gmap: &Gmap, key: &str) -> (SpanRef, Option<String>, Option<String>) {
    let hits: Vec<&GmapNode> = gmap.nodes.iter().filter(|n| n.key == key).collect();
    assert_eq!(hits.len(), 1, "{key} must export exactly once");
    match &hits[0].content {
        Content::Symbol { span, doc, .. } => (*span, doc.clone(), hits[0].identity_hint.clone()),
        other => panic!("{key}: expected Symbol, got {other:?}"),
    }
}

#[test]
fn duplicate_key_prefers_located_node() {
    let root = std::env::temp_dir();
    const KEY: &str = "svc::orders::OrderService::validate";
    // The ATTRIBUTE twin has the lower id, so only the POSITION can beat it.
    let attr = (1, KEY, node_kind::ATTRIBUTE, vec![]);
    let method = (
        2,
        KEY,
        node_kind::METHOD,
        vec![pos("svc/orders.py", 10, 11), doc("Check a sku.")],
    );
    for rows in [vec![attr.clone(), method.clone()], vec![method, attr]] {
        let (gmap, _, stats) = build_gmap(&graph(&rows), &root, &ExportOptions::default());
        let (span, doc, hint) = symbol(&gmap, KEY);
        assert_eq!((span.start_line, span.end_line), (11, 12));
        assert_eq!(doc.as_deref(), Some("Check a sku."));
        assert_eq!(
            hint,
            Some(format!("svc/orders.py:{}:0", node_kind::METHOD.0))
        );
        assert_eq!(stats.duplicate_keys, 1);
        assert_eq!(stats.identity_hints, 1);
    }
}

#[test]
fn duplicate_key_prefers_the_declaration_over_its_module() {
    // A Java public class shares its file MODULE's qname (LB.2; PHP, Scala
    // and C# likewise): both are located, and the declaration is kept even
    // when the MODULE has the lower id and a DOC cell.
    let root = std::env::temp_dir();
    const KEY: &str = "app::Widget";
    let module = (
        1,
        KEY,
        node_kind::MODULE,
        vec![pos("app/Widget.java", 0, 9), doc("File header.")],
    );
    let class = (2, KEY, node_kind::CLASS, vec![pos("app/Widget.java", 2, 9)]);
    for rows in [vec![module.clone(), class.clone()], vec![class, module]] {
        let (gmap, _, stats) = build_gmap(&graph(&rows), &root, &ExportOptions::default());
        let (span, _, hint) = symbol(&gmap, KEY);
        assert_eq!(span.start_line, 3);
        assert_eq!(
            hint,
            Some(format!("app/Widget.java:{}:0", node_kind::CLASS.0))
        );
        assert_eq!(stats.duplicate_keys, 1);
    }
}

#[test]
fn file_identity_replaces_the_path_in_hints() {
    let root = std::env::temp_dir();
    let rows = [
        (
            1,
            "svc::core::orders",
            node_kind::MODULE,
            vec![pos("svc/core/orders.py", 0, 3)],
        ),
        (
            2,
            "svc::core::orders::f",
            node_kind::FUNCTION,
            vec![pos("svc/core/orders.py", 1, 2)],
        ),
        (
            3,
            "svc::other",
            node_kind::MODULE,
            vec![pos("svc/other.py", 0, 1)],
        ),
    ];
    let opts = ExportOptions {
        file_identity: [(
            "svc/core/orders.py".to_string(),
            "svc/orders.py".to_string(),
        )]
        .into(),
        ..Default::default()
    };
    let (gmap, _, _) = build_gmap(&graph(&rows), &root, &opts);
    assert_eq!(
        hint(&gmap, "svc::core::orders"),
        format!("svc/orders.py:{}:0", node_kind::MODULE.0)
    );
    assert_eq!(
        hint(&gmap, "svc::core::orders::f"),
        format!("svc/orders.py:{}:0", node_kind::FUNCTION.0)
    );
    // A path the map does not name keeps its path.
    assert_eq!(
        hint(&gmap, "svc::other"),
        format!("svc/other.py:{}:0", node_kind::MODULE.0)
    );
}
