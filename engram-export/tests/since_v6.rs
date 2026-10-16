//! LG.8a: `--since <prior>` writes the G16 diff `<out>.diff` beside the full
//! gmap, file ids are seeded from the prior (a path that survives keeps its
//! id), and the default build is the persisted incremental one, whose
//! parse-cache counts (LA.12 `last_diff`) the `v6 since:` marker reports.
//!
//! Every test runs the built bin on a scratch dir (no git: LB.6 pairs moves by
//! content and name), with every `--out` outside the exported repo.
//!
//! CK.3: the bin names the glia that built it - `-V`, the `glia build=`
//! marker, the `<out>.meta.json` sidecar and the history manifest's
//! `build_stamp`.
//!
//! DE.3: an ENDPOINT (a client call, no file of its own) is anchored at its
//! call site, so an endpoint re-keyed by a path edit arrives as a move.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use engram_core::{EdgeKind, GMAP_FORMAT_VERSION, GmapDiff, content_digest};
use glia_engram_export::diff::diff_path;
use glia_engram_export::{META_VERSION, history_dir, meta_path};

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

/// The `[engram-export] v6 since:` counts for the A -> B edit. DT.10a: the
/// public `balances` getter and `deposit` are RPC_PROCEDUREs `evm:Vault.<m>`,
/// whose qnames name no file, so the move leaves them in place, modified
/// location-only (8 -> 10, 7 -> 9).
const B_COUNTS: &str =
    "added=1 removed=1 modified=10 (moved=4 location_only=9) edges +1/-0; parse cache ";

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
        "[engram-export] v6 since pairing: route=0 endpoint=0 name=4 words=0 hint=0 hint_refused=0"
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
        same("evm:Vault.balances", true),
        same("evm:Vault.deposit", true),
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
        "[engram-export] v6 since pairing: route=0 endpoint=0 name=0 words=0 hint=0 hint_refused=0"
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

/// The one `[engram-export] glia build=` marker line (CK.3).
fn build_line(stderr: &str) -> &str {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[engram-export] glia build="))
        .collect();
    assert_eq!(lines.len(), 1, "want one glia build marker in:\n{stderr}");
    lines[0]
}

/// The `<out>.meta.json` sidecar beside `out`: its raw text and its JSON.
fn read_meta(out: &std::path::Path) -> (String, serde_json::Value) {
    let path = meta_path(out);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no meta sidecar at {}: {e}", path.display()));
    let value = serde_json::from_str(&text).unwrap();
    (text, value)
}

#[test]
fn version_and_history_name_the_glia_build() {
    let bin = env!("CARGO_BIN_EXE_glia-export-engram");
    // (a) -V and --version print glia's version line, build stamp included.
    for flag in ["-V", "--version"] {
        let out = Command::new(bin).arg(flag).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{flag}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("glia-export-engram {}\n", glia_engine::VERSION_LINE),
            "{flag}"
        );
    }

    // (b) every run prints the build marker once, naming the meta sidecar.
    let s = Scratch::new("stamp");
    let a_err = s.export("a", None, &["--no-persist"], false);
    let meta_a = meta_path(&s.out("a"));
    assert_eq!(
        build_line(&a_err),
        format!(
            "[engram-export] glia build={} exporter={} format_version={} meta={}",
            glia_engine::BUILD_STAMP,
            env!("CARGO_PKG_VERSION"),
            GMAP_FORMAT_VERSION,
            meta_a.display()
        )
    );

    // (c) the history layout's manifest records the same build.
    let manifest = std::fs::read_to_string(history_dir(&s.out("a")).join("manifest.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    assert_eq!(manifest["build_stamp"], glia_engine::BUILD_STAMP);

    // (d) the meta sidecar: exactly these keys, sorted, bound to the gmap.
    let (text, meta) = read_meta(&s.out("a"));
    let keys: Vec<&str> = meta.as_object().unwrap().keys().map(String::as_str).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        [
            "build_stamp",
            "exporter",
            "exporter_version",
            "format_version",
            "glia_version",
            "gmap_digest",
            "meta_version",
            "parser_stamp",
            "version_line",
        ]
    );
    assert!(text.starts_with("{\n  \"build_stamp\""), "keys not sorted:\n{text}");
    let a_digest = hex(content_digest(&s.bytes("a")));
    assert_eq!(meta["build_stamp"], glia_engine::BUILD_STAMP);
    assert_eq!(meta["exporter"], "glia-export-engram");
    assert_eq!(meta["exporter_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(meta["format_version"], GMAP_FORMAT_VERSION);
    assert_eq!(meta["glia_version"], glia_engine::RELEASE);
    assert_eq!(meta["gmap_digest"], a_digest.as_str());
    assert_eq!(meta["meta_version"], META_VERSION);
    assert_eq!(META_VERSION, 1);
    assert_eq!(meta["parser_stamp"], glia_engine::PARSER_STAMP);
    assert_eq!(meta["version_line"], glia_engine::VERSION_LINE);
    assert!(
        a_err.contains(&format!(" digest={a_digest}\n")),
        "the wrote line's digest is the meta's:\n{a_err}"
    );

    // (e) an identical export writes a byte-identical meta; a --since export
    // writes its own, bound to its own gmap (the since line's target).
    s.export("a", None, &["--no-persist"], false);
    assert_eq!(read_meta(&s.out("a")).0, text, "identical exports, different meta");
    s.edit_a_to_b();
    let b_err = s.export("b", Some("a"), &["--no-persist"], false);
    let b_digest = hex(content_digest(&s.bytes("b")));
    assert_ne!(b_digest, a_digest);
    assert!(
        since_line(&b_err).contains(&format!(" target={b_digest} ")),
        "{b_err}"
    );
    assert_eq!(read_meta(&s.out("b")).1["gmap_digest"], b_digest.as_str());
    assert!(
        build_line(&b_err).ends_with(&format!(" meta={}", meta_path(&s.out("b")).display())),
        "{b_err}"
    );

    // A refused run (an unreadable --since prior) writes no meta.
    let out = Command::new(bin)
        .arg(s.repo())
        .arg("--out")
        .arg(s.out("c"))
        .arg("--since")
        .arg(s.out("never-written"))
        .arg("--no-persist")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert!(!meta_path(&s.out("c")).exists(), "a refused run wrote a meta");
}

/// An Angular service with one HttpClient call (DE.3).
const STATS_SERVICE_TS: &str = "import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';

@Injectable({ providedIn: 'root' })
export class StatsService {
  constructor(private http: HttpClient) {}

  load() {
    return this.http.get('/stats');
  }
}
";

/// The one `[engram-export] v6 identity: <n>/<m> hints, ...` marker line.
fn identity_line(stderr: &str) -> &str {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[engram-export] v6 identity: ") && l.contains(" hints, "))
        .collect();
    assert_eq!(lines.len(), 1, "want one v6 identity marker in:\n{stderr}");
    lines[0]
}

/// DE.3: the call's path gains an `/api` prefix, re-keying its ENDPOINT
/// `endpoint:GET:/stats` -> `endpoint:GET:/api/stats`. Both are anchored at
/// the call site in the service file, so the route pass pairs them by verb
/// and alike path: one move, not a removed + an added fact.
#[test]
fn a_rekeyed_endpoint_arrives_as_a_move() {
    let s = Scratch::new("endpoint");
    s.write("web/stats.service.ts", STATS_SERVICE_TS);
    let a_err = s.export("a", None, &["--no-persist"], false);
    assert!(
        identity_line(&a_err).ends_with("; anchored endpoint=1/1 page=0/0"),
        "{a_err}"
    );
    let edited = s.read("web/stats.service.ts").replace("'/stats'", "'/api/stats'");
    s.write("web/stats.service.ts", &edited);
    let b_err = s.export("b", Some("a"), &["--no-persist"], false);
    assert_eq!(
        pairing_line(&b_err),
        "[engram-export] v6 since pairing: route=0 endpoint=1 name=0 words=0 hint=0 hint_refused=0"
    );

    let diff = s.diff("b");
    let moved: Vec<(&str, &str)> = diff
        .modified
        .iter()
        .filter(|c| c.prior_key != c.node.key)
        .map(|c| (c.prior_key.as_str(), c.node.key.as_str()))
        .collect();
    assert_eq!(moved, [("endpoint:GET:/stats", "endpoint:GET:/api/stats")]);
    let change = diff
        .modified
        .iter()
        .find(|c| c.prior_key == "endpoint:GET:/stats")
        .unwrap();
    let kind = glia_code_domain::node_kind::ENDPOINT.0;
    assert_eq!(
        change.node.identity_hint.as_deref(),
        Some(format!("web/stats.service.ts:{kind}:0").as_str())
    );
    assert!(
        !diff.removed.iter().any(|k| k.starts_with("endpoint:")),
        "{:?}",
        diff.removed
    );
    assert!(
        !diff.added.iter().any(|n| n.key.starts_with("endpoint:")),
        "{:?}",
        diff.added.iter().map(|n| &n.key).collect::<Vec<_>>()
    );
}
