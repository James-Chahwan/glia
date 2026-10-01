//! LG.8a: `--since <prior>` writes the G16 diff `<out>.diff` beside the full
//! gmap, file ids are seeded from the prior (a path that survives keeps its
//! id), and the default build is the persisted incremental one, whose
//! parse-cache counts (LA.12 `last_diff`) the `v6 since:` marker reports.
//!
//! Every test runs the built bin on a scratch dir (no git: LB.6 pairs moves by
//! content and name), with every `--out` outside the exported repo.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use engram_core::{EdgeKind, GmapDiff, content_digest};
use glia_engram_export::diff::diff_path;

/// The LG.7 probe fixture (12, 7 and 12 lines). `Vault.sol` is
/// NatSpec-neutral: no contract comment and one untagged line on `deposit`, so
/// it yields a doc and no tag facts whatever LG.12 exports.
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

/// The `[engram-export] v6 since:` counts for the A -> B edit.
const B_COUNTS: &str =
    "added=1 removed=1 modified=8 (moved=4 location_only=7) edges +1/-0; parse cache ";

/// A scratch dir holding the repo (`fx/`) and the exports (`out/`), removed
/// on drop. The repo dir is `fx` in every scratch, so every run has one repo
/// identity (`dir:fx`) and two scratches export equal bytes.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-lg8a-{}-{test}", std::process::id()));
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

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.repo().join(rel)).unwrap()
    }

    /// The A -> B edit: `place`'s docstring changes at equal length, `cancel`
    /// is appended after `validate`, the `# Orders` section is deleted and
    /// `Vault.sol` moves to `contracts/core/`.
    fn edit_a_to_b(&self) {
        let orders = self.read("svc/orders.py");
        let edited = orders.replace("Place one order.", "Place any order.")
            + "\n    def cancel(self, sku):\n        return None\n";
        self.write("svc/orders.py", &edited);
        let readme = self.read("docs/README.md");
        let placing = readme.find("## Placing orders").unwrap();
        self.write("docs/README.md", &readme[placing..]);
        let to = self.repo().join("contracts/core/Vault.sol");
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::rename(self.repo().join("contracts/Vault.sol"), to).unwrap();
    }

    /// Run the bin: export to `out/<name>`, `--since out/<since>` when given,
    /// the `extra` flags after. `GLIA_NO_PERSIST` is cleared unless `env_off`
    /// sets it, so an inherited value cannot pick the build. Asserts exit 0
    /// and returns stderr.
    fn export(&self, name: &str, since: Option<&str>, extra: &[&str], env_off: bool) -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia-export-engram"));
        cmd.arg(self.repo()).arg("--out").arg(self.out(name));
        if let Some(prior) = since {
            cmd.arg("--since").arg(self.out(prior));
        }
        cmd.args(extra).env_remove("GLIA_NO_PERSIST");
        if env_off {
            cmd.env("GLIA_NO_PERSIST", "1");
        }
        let out = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(
            out.status.code(),
            Some(0),
            "export {name} failed:\n{stderr}"
        );
        stderr
    }

    fn bytes(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.out(name)).unwrap()
    }

    fn diff_bytes(&self, name: &str) -> Vec<u8> {
        let path = diff_path(&self.out(name));
        std::fs::read(&path).unwrap_or_else(|e| panic!("no diff at {}: {e}", path.display()))
    }

    fn diff(&self, name: &str) -> GmapDiff {
        bincode::deserialize(&self.diff_bytes(name)).unwrap()
    }

    fn cache_file(&self) -> PathBuf {
        self.repo().join(".glia/graph/parse_cache.bin")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The one `[engram-export] v6 since: base=` marker line.
fn since_line(stderr: &str) -> &str {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[engram-export] v6 since: base="))
        .collect();
    assert_eq!(lines.len(), 1, "want one v6 since marker in:\n{stderr}");
    lines[0]
}

/// The one `[engram-export] v6 since pairing:` line (CK.2).
fn pairing_line(stderr: &str) -> &str {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[engram-export] v6 since pairing:"))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "want one v6 since pairing line in:\n{stderr}"
    );
    lines[0]
}

fn hex(digest: u64) -> String {
    format!("{digest:016x}")
}

/// `prior_key -> (key, location_only)` over `diff.modified`.
fn modified(diff: &GmapDiff) -> BTreeMap<String, (String, bool)> {
    diff.modified
        .iter()
        .map(|c| (c.prior_key.clone(), (c.node.key.clone(), c.location_only)))
        .collect()
}

fn files(pairs: &[(u32, &str)]) -> BTreeMap<u32, String> {
    pairs.iter().map(|(id, p)| (*id, p.to_string())).collect()
}

/// Export A, apply the A -> B edit and export B `--since A`, persisted or not
/// (`--no-persist` on B; `GLIA_NO_PERSIST=1` on A). Returns B's stderr.
fn a_to_b(s: &Scratch, persist: bool) -> String {
    let a_err = s.export("a", None, &[], !persist);
    assert!(
        !a_err.contains("[engram-export] v6 since:"),
        "no --since, no since marker:\n{a_err}"
    );
    assert!(!diff_path(&s.out("a")).exists(), "no --since, no diff");
    assert_eq!(s.cache_file().is_file(), persist, "{a_err}");
    s.edit_a_to_b();
    let flags: &[&str] = if persist { &[] } else { &["--no-persist"] };
    s.export("b", Some("a"), flags, false)
}

#[test]
fn since_writes_the_diff_and_seeds_file_ids() {
    let s = Scratch::new("diff");
    let b_err = a_to_b(&s, true);
    let diff = s.diff("b");

    let (a_digest, b_digest) = (content_digest(&s.bytes("a")), content_digest(&s.bytes("b")));
    assert_eq!(diff.base_digest, a_digest);
    assert_eq!(diff.target_digest, b_digest);
    assert_eq!(
        since_line(&b_err),
        format!(
            "[engram-export] v6 since: base={} target={} {B_COUNTS}reused=0 reparsed=2 evicted=1",
            hex(a_digest),
            hex(b_digest)
        )
    );
    // The four Vault nodes of the moved Vault.sol keep their file token, kind
    // and name: they pair by name, before their hints.
    assert_eq!(
        pairing_line(&b_err),
        "[engram-export] v6 since pairing: route=0 name=4 hint=0 hint_refused=0"
    );

    let added: Vec<&str> = diff.added.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(added, ["svc::orders::OrderService::cancel"]);
    assert_eq!(diff.removed, ["docs::docs::README::orders"]);
    let vault = |old: &str| {
        (
            format!("contracts::Vault{old}"),
            (format!("contracts::core::Vault{old}"), true),
        )
    };
    let same = |key: &str, location_only: bool| (key.to_string(), (key.to_string(), location_only));
    let want: BTreeMap<String, (String, bool)> = [
        same("svc::orders::OrderService::place", false),
        same("svc::orders", true),
        same("svc::orders::OrderService", true),
        same("docs::docs::README::placing-orders", true),
        vault(""),
        vault("::Vault"),
        vault("::Vault::balances"),
        vault("::Vault::deposit"),
    ]
    .into_iter()
    .collect();
    assert_eq!(modified(&diff), want);

    let edge = |e: &engram_core::GmapEdge| (e.from.clone(), e.kind, e.to.clone());
    let edges_added: Vec<_> = diff.edges_added.iter().map(edge).collect();
    assert_eq!(
        edges_added,
        [(
            "svc::orders::OrderService".to_string(),
            EdgeKind::Contains,
            "svc::orders::OrderService::cancel".to_string()
        )]
    );
    assert!(diff.edges_removed.is_empty(), "{:?}", diff.edges_removed);
    // Surviving paths keep A's ids; the moved file is new at max + 1.
    assert_eq!(
        diff.files,
        files(&[
            (2, "docs/README.md"),
            (3, "svc/orders.py"),
            (4, "contracts/core/Vault.sol")
        ])
    );

    // C: no edits since B. The diff is still written, and empty.
    let c_err = s.export("c", Some("b"), &[], false);
    assert_eq!(
        since_line(&c_err),
        format!(
            "[engram-export] v6 since: base={0} target={0} unchanged added=0 removed=0 modified=0 \
             (moved=0 location_only=0) edges +0/-0; parse cache reused=2 reparsed=0 evicted=0",
            hex(b_digest)
        )
    );
    assert_eq!(
        pairing_line(&c_err),
        "[engram-export] v6 since pairing: route=0 name=0 hint=0 hint_refused=0"
    );
    assert_eq!(s.bytes("c"), s.bytes("b"));
    let c = s.diff("c");
    assert_eq!((c.base_digest, c.target_digest), (b_digest, b_digest));
    assert!(c.added.is_empty() && c.removed.is_empty() && c.modified.is_empty());
    assert!(c.edges_added.is_empty() && c.edges_removed.is_empty());
    assert_eq!(c.files, diff.files);

    // A full export over --out drops the diff a --since run left beside it:
    // that diff no longer reaches the gmap at --out.
    s.export("c", None, &[], false);
    assert!(!diff_path(&s.out("c")).exists(), "stale diff kept beside c");
}

#[test]
fn no_persist_writes_the_same_diff_and_no_cache() {
    let persisted = Scratch::new("persist");
    a_to_b(&persisted, true);
    let clean = Scratch::new("clean");
    let b_err = a_to_b(&clean, false);
    assert!(
        since_line(&b_err).ends_with(&format!("{B_COUNTS}off")),
        "{b_err}"
    );
    assert!(
        !clean.repo().join(".glia").exists(),
        "--no-persist wrote build state"
    );
    assert_eq!(clean.bytes("b"), persisted.bytes("b"));
    assert_eq!(clean.diff_bytes("b"), persisted.diff_bytes("b"));
}

#[test]
fn a_file_sorting_first_renumbers_nothing() {
    let s = Scratch::new("first");
    s.export("a", None, &["--no-persist"], false);
    s.write("a/first.py", "def first():\n    return 1\n");
    s.export("b", Some("a"), &["--no-persist"], false);
    let diff = s.diff("b");
    assert_eq!(
        diff.files,
        files(&[
            (1, "contracts/Vault.sol"),
            (2, "docs/README.md"),
            (3, "svc/orders.py"),
            (4, "a/first.py"),
        ])
    );
    // Only the new file's nodes: no untouched node's span.file moved.
    assert!(diff.modified.is_empty(), "{:?}", modified(&diff));
    assert!(diff.removed.is_empty(), "{:?}", diff.removed);
    let added: BTreeSet<&str> = diff.added.iter().map(|n| n.key.as_str()).collect();
    assert!(added.iter().all(|k| k.starts_with("a::first")), "{added:?}");
    assert!(added.contains("a::first::first"), "{added:?}");
}
