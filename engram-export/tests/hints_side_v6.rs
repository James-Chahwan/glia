//! T12: every export writes `<out>.hints.jsonl` beside the gmap - a header
//! naming the gmap's content digest, then one `{"hint","key"}` row per fact,
//! sorted by key, `hint` being the LB.6 `Identity::hint` v1 string of the
//! fact's glia node (the form glia's cell store keys a row by) or null - and
//! changes nothing else the export writes.
//!
//! The bin tests run the built bin on a scratch dir outside git, with every
//! `--out` outside the exported repo, and `--no-persist` so no parse cache is
//! written into the fixture. The one-format test runs in process: the hints
//! the export names are the ones a cell write computes on a persisted layout,
//! and each binds its fact through the cell store's binder.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use engram_core::{Content, Gmap, content_digest};
use glia_code_domain::node_kind;
use glia_engine::generate_one;
use glia_engine::persist::{load_layout, persist_graph};
use glia_engram_export::hints_side::hint_rows;
use glia_engram_export::{
    ExportOptions, build_gmap, build_gmap_with_nodes, hints_side_path, meta_path, sidecar_path,
};
use glia_graph::cells::{CellTarget, QnameIndex};
use glia_graph::identity::{Identity, identity_of};

/// A call chain `a -> b -> c` beside an unreached `d`, plus a class whose
/// method body is long enough
/// (>= 40 whitespace-collapsed chars) to carry a body hash.
const CHAIN_PY: &str = "def a():
    return b()


def b():
    return c()


def c():
    return 1


def d():
    return 0


class Ledger:
    def balance(self, entries):
        return sum(e.amount for e in entries if e.posted)
";

/// The METHOD / ATTRIBUTE twin: the `self.total` read mints an ATTRIBUTE with
/// the METHOD's qname `svc::cart::Cart::total`, so the qname alone is
/// ambiguous to a cell write and the export's key winner is the METHOD.
const CART_PY: &str = "class Cart:
    def total(self): return sum(item.price * item.quantity for item in self.items)
    def summary(self): handler = self.total; return handler()
";

/// A client call: a positionless ENDPOINT, which has no v1 identity.
const API_TS: &str = "export async function load() { return fetch('/api/orders'); }
";

/// One doc section: a Proposition with no v1 identity.
const README_MD: &str = "# Chain

## Entry

Call `a` to run the chain.
";

const MARKER: &str = "[engram-export] hints side: ";

/// `Cart.total`'s CODE hash, measured with `glia cell set .. --kind METHOD`
/// on a one-file copy of `CART_PY` (the hash reads the CODE text, never the
/// file name).
const CART_TOTAL_BODY: &str = "8285f8265b86a93e";

struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-dt12a-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, body) in [
            ("fx/svc/chain.py", CHAIN_PY),
            ("fx/svc/cart.py", CART_PY),
            ("fx/web/api.ts", API_TS),
            ("fx/docs/README.md", README_MD),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        std::fs::create_dir_all(root.join("out")).unwrap();
        Scratch(root)
    }

    fn fx(&self) -> PathBuf {
        self.0.join("fx")
    }

    fn out(&self, name: &str) -> PathBuf {
        self.0.join("out").join(format!("{name}.engram-gmap"))
    }

    fn hints(&self, name: &str) -> PathBuf {
        hints_side_path(&self.out(name))
    }

    fn run(&self, name: &str, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glia-export-engram"))
            .arg(self.fx())
            .arg("--out")
            .arg(self.out(name))
            .arg("--no-persist")
            .args(extra)
            .env_remove("GLIA_NO_PERSIST")
            .output()
            .unwrap()
    }

    /// Export to `out/<name>`, asserting exit 0; returns stderr.
    fn export(&self, name: &str, extra: &[&str]) -> String {
        let out = self.run(name, extra);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(0), "export {name} failed:\n{stderr}");
        assert!(!self.fx().join(".glia").exists(), "--no-persist wrote into the fixture");
        stderr
    }

    fn gmap(&self, name: &str) -> Gmap {
        bincode::deserialize(&read(&self.out(name))).unwrap()
    }

    /// The hints file's header and rows, each line parsed.
    fn hint_lines(&self, name: &str) -> (serde_json::Value, Vec<serde_json::Value>) {
        let text = String::from_utf8(read(&self.hints(name))).unwrap();
        assert!(text.ends_with('\n'), "JSONL ends with a newline");
        let mut lines = text.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap());
        let header = lines.next().expect("a header line");
        (header, lines.collect())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `(key, hint)` per row.
fn rows(lines: &[serde_json::Value]) -> Vec<(String, Option<String>)> {
    lines
        .iter()
        .map(|r| {
            let obj = r.as_object().unwrap();
            assert_eq!(obj.len(), 2, "a row is {{hint, key}}: {r}");
            (r["key"].as_str().unwrap().to_string(), r["hint"].as_str().map(str::to_string))
        })
        .collect()
}

fn hint_of<'a>(rows: &'a [(String, Option<String>)], key: &str) -> Option<&'a str> {
    rows.iter().find(|r| r.0 == key).unwrap_or_else(|| panic!("no row for {key}")).1.as_deref()
}

#[test]
fn side_file_binds_to_its_gmap_and_lists_every_fact() {
    let s = Scratch::new("bound");
    let stderr = s.export("a", &[]);
    let gmap = s.gmap("a");
    let meta: serde_json::Value = serde_json::from_slice(&read(&meta_path(&s.out("a")))).unwrap();
    let digest = format!("{:016x}", content_digest(&read(&s.out("a"))));
    assert_eq!(meta["gmap_digest"], digest);

    let (header, lines) = s.hint_lines("a");
    let rows = rows(&lines);
    let hinted = rows.iter().filter(|r| r.1.is_some()).count();
    assert_eq!(
        header,
        serde_json::json!({
            "facts": gmap.nodes.len(),
            "gmap_digest": digest,
            "hint_version": "v1",
            "hinted": hinted,
            "hints_side": 1,
        })
    );

    // One row per fact, strictly ascending by key, the gmap's keys exactly.
    assert!(rows.windows(2).all(|w| w[0].0 < w[1].0), "rows sorted by key, no repeat");
    let row_keys: BTreeSet<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    let gmap_keys: BTreeSet<&str> = gmap.nodes.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(row_keys, gmap_keys);
    assert_eq!(rows.len(), gmap.nodes.len());

    // Every hint is a well-formed v1 hint; some carry a body hash.
    for (key, hint) in &rows {
        if let Some(h) = hint {
            assert_eq!(Identity::parse_hint(h).map(|i| i.hint()).as_deref(), Some(h.as_str()), "{key}");
        }
    }
    assert!(hinted > 0 && hinted < rows.len(), "{hinted} of {}", rows.len());
    assert!(
        rows.iter().filter_map(|r| r.1.as_deref()).any(|h| !h.contains("|0000000000000000|")),
        "some fact has a body hash"
    );

    // The client call's ENDPOINT and the doc section have no v1 identity.
    let endpoint = rows.iter().find(|r| r.0.starts_with("endpoint:")).expect("an endpoint: fact");
    assert_eq!(endpoint.1, None, "{}", endpoint.0);
    let section = gmap
        .nodes
        .iter()
        .find(|n| matches!(n.content, Content::Proposition { .. }))
        .expect("a doc section fact");
    assert_eq!(hint_of(&rows, &section.key), None, "{}", section.key);
    // A function with a MODULE ancestor and a file has one.
    let a = rows.iter().find(|r| r.0.ends_with("svc::chain::a")).expect("chain::a");
    assert_eq!(a.1.as_deref(), Some(format!("v1|{}|chain.py|0000000000000000|a", node_kind::FUNCTION.0).as_str()));

    // The marker: exactly one line, the counts, and the file's path.
    let marker: Vec<&str> = stderr.lines().filter(|l| l.starts_with(MARKER)).collect();
    assert_eq!(marker.len(), 1, "{stderr}");
    let want = format!("{MARKER}{} facts, {hinted} v1 hints, ", gmap.nodes.len());
    assert!(marker[0].starts_with(&want), "{} vs {want}", marker[0]);
    assert!(marker[0].ends_with(&format!(" ms -> {}", s.hints("a").display())), "{}", marker[0]);
    // After the build marker, before anything the run prints later.
    let at = |prefix: &str| stderr.lines().position(|l| l.starts_with(prefix)).expect(prefix);
    assert!(at("[engram-export] glia build=") < at(MARKER), "{stderr}");
}

#[test]
fn hints_bind_their_fact_on_the_layout_a_cell_write_reads() {
    let s = Scratch::new("bind");
    let r = generate_one(s.fx().to_str().unwrap()).expect("build");
    let (gmap, _, _, fact_nodes) = build_gmap_with_nodes(&r.merged, &s.fx(), &ExportOptions::default());
    assert_eq!(fact_nodes.len(), gmap.nodes.len(), "one node slot per fact");
    // build_gmap is the same gmap, without the node slots.
    let plain = build_gmap(&r.merged, &s.fx(), &ExportOptions::default()).0;
    assert_eq!(bincode::serialize(&plain).unwrap(), bincode::serialize(&gmap).unwrap());

    let rows = hint_rows(&r.merged, &gmap, &fact_nodes);
    let row_hint = |key: &str| rows.iter().find(|row| row.key == key).and_then(|row| row.hint.clone());
    let facts: Vec<(&str, glia_core::NodeId)> = gmap
        .nodes
        .iter()
        .zip(&fact_nodes)
        .filter_map(|(n, id)| id.map(|id| (n.key.as_str(), id)))
        .collect();
    assert_eq!(facts.len(), gmap.nodes.len(), "a Python / TS / markdown export has no NatSpec tag fact");
    for (key, id) in &facts {
        assert_eq!(row_hint(key), identity_of(&r.merged, *id).map(|i| i.hint()), "{key}");
    }

    // A rooted layout stores CODE as spans (CD.7c); the cell store's fresh
    // layout read rehydrates them. Every hint survives the round trip.
    let dir = s.0.join("layout");
    persist_graph(&r.merged, &r.repo_labels, &r.repo_roots, &r.parse_errors, &dir, "test").expect("persist");
    let loaded = load_layout(&dir).expect("load").merged;
    let idx = QnameIndex::build(&loaded, None);
    let mut bound = 0;
    for (key, id) in &facts {
        let hint = row_hint(key);
        assert_eq!(identity_of(&loaded, *id).map(|i| i.hint()), hint, "{key} after the layout round trip");
        if let Some(h) = hint {
            assert_eq!(idx.resolve(key, None, Some(&h)), CellTarget::Bound(*id), "{key} {h}");
            bound += 1;
        }
    }
    assert_eq!(bound, rows.iter().filter(|row| row.hint.is_some()).count());

    // The twin: the qname alone is ambiguous, the hint alone binds the METHOD.
    let (cart, cart_id) = *facts.iter().find(|(k, _)| k.ends_with("svc::cart::Cart::total")).expect("Cart.total");
    assert!(matches!(idx.resolve(cart, None, None), CellTarget::Ambiguous(_)), "two nodes carry {cart}");
    let hint = row_hint(cart).expect("Cart.total has a v1 hint");
    let ident = Identity::parse_hint(&hint).expect("v1");
    assert_eq!(ident.kind, node_kind::METHOD, "the export's key winner is the METHOD: {hint}");
    assert_eq!(hint, format!("v1|{}|cart.py|{CART_TOTAL_BODY}|Cart::total", node_kind::METHOD.0));
    assert_eq!(idx.resolve(cart, None, Some(&hint)), CellTarget::Bound(cart_id));
    assert_eq!(idx.resolve(cart, Some("METHOD"), None), CellTarget::Bound(cart_id));
}

#[test]
fn rerun_and_flags_change_nothing() {
    let s = Scratch::new("rerun");
    s.export("a", &[]);
    let first = read(&s.hints("a"));
    s.export("a", &[]);
    assert_eq!(read(&s.hints("a")), first, "a rerun writes the same hints bytes");
    // 0.5.2's `--ppr-candidates` (DE.5) is gone (EG.4): clap refuses it.
    let refused = s.run("ppr", &["--ppr-candidates", "3"]);
    assert_eq!(refused.status.code(), Some(2), "--ppr-candidates is an unexpected argument");
    // A stale hints file is replaced, never appended to or kept; a stale
    // `.ppr.jsonl` a 0.5.2 run left beside the gmap is deleted.
    let stale_ppr = {
        let mut p = s.out("stale").into_os_string();
        p.push(".ppr.jsonl");
        PathBuf::from(p)
    };
    std::fs::write(s.hints("stale"), b"garbage\n").unwrap();
    std::fs::write(&stale_ppr, b"{\"stale\":true}\n").unwrap();
    s.export("stale", &[]);
    assert_eq!(read(&s.hints("stale")), first);
    assert!(!stale_ppr.exists(), "a stale {} is removed", stale_ppr.display());
    // The gmap and its sidecars are the same bytes whatever the flags.
    let files = |name: &str| [s.out(name), sidecar_path(&s.out(name)), meta_path(&s.out(name))];
    for name in ["stale"] {
        for (a, b) in files("a").into_iter().zip(files(name)) {
            assert_eq!(read(&a), read(&b), "{} differs from {}", b.display(), a.display());
        }
    }
}
