//! LD.5 — `glia blast-radius <REPO> <QNAME>...`: many seeds, one radius.
//!
//! Before LD.5 the command took exactly one qname: `glia blast-radius api
//! app::save app::publish` was clap's `error: unexpected argument
//! 'app::publish' found` (exit 2, measured at the wave HEAD). Now it is one
//! walk and one ranking over both seeds, each row naming its seed, and an
//! unknown name is reported instead of failing the whole answer: exit 3 only
//! when every name is unknown.
//!
//! Drives the real binary. The `[blast] seeds=` stderr line is the LD.5
//! fired_on marker; asserting it here makes it a tested contract.

use std::path::PathBuf;
use std::process::{Command, Output};

const APP_PY: &str = "from flask import Flask\nfrom kafka import KafkaProducer\n\napp = Flask(__name__)\nproducer = KafkaProducer()\n\n\ndef audit(order):\n    return order\n\n\ndef publish(order):\n    producer.send('orders', order)\n\n\ndef save(order):\n    return audit(order)\n\n\n@app.route('/orders', methods=['POST'])\ndef create_order():\n    order = {}\n    save(order)\n    publish(order)\n    return order\n";

/// A fresh `api` repo dir holding only `app.py`, removed on drop.
struct TempRepo(PathBuf);

impl TempRepo {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-ld5-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let api = root.join("api");
        std::fs::create_dir_all(&api).expect("temp dir");
        std::fs::write(api.join("app.py"), APP_PY).expect("write app.py");
        TempRepo(root)
    }

    fn api(&self) -> String {
        self.0.join("api").to_str().expect("temp path is UTF-8").to_string()
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn glia(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_glia"))
        .env("GLIA_NO_PERSIST", "1")
        .args(args)
        .output()
        .expect("glia runs")
}

/// The one `[blast] seeds=` line of a run, relayed so `-- --nocapture 2>&1 |
/// grep '\[blast\] seeds='` sees it.
fn marker(out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().filter(|l| l.starts_with("[blast] seeds=")).collect();
    assert_eq!(lines.len(), 1, "one [blast] line per answer:\n{stderr}");
    eprintln!("{}", lines[0]);
    lines[0].to_string()
}

#[test]
fn several_qnames_are_one_radius() {
    let repo = TempRepo::new("json");
    let api = repo.api();
    let out = glia(&["blast-radius", &api, "app::save", "app::publish", "--json"]);
    assert!(out.status.success(), "exit {:?}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let seeds: Vec<&str> =
        v["seeds"].as_array().expect("seeds").iter().filter_map(|s| s["qname"].as_str()).collect();
    assert_eq!(seeds, ["app::save", "app::publish"], "{v}");
    assert_eq!(v["unresolved"], serde_json::json!([]), "{v}");
    assert!(v["absence"].is_null(), "{v}");
    // EA.5: the answer's keys (sorted: serde_json without preserve_order)
    // gain `files`, the measured file ranking;
    // the seeds' own file is never listed.
    let keys: Vec<&str> = v.as_object().expect("object").keys().map(String::as_str).collect();
    assert_eq!(keys, ["absence", "files", "results", "seeds", "unresolved"], "{v}");
    assert!(v["files"].as_array().expect("files").iter().all(|f| f["file"] != "app.py"), "{v}");
    let rows = v["results"].as_array().expect("results");
    let create = rows.iter().find(|r| r["qname"] == "app::create_order").expect("the shared caller");
    assert_eq!(create["seed"], "app::save", "the first seed's wave reaches it first: {v}");
    assert!(rows.iter().all(|r| r["qname"] != "app::save" && r["qname"] != "app::publish"), "{v}");
    assert!(marker(&out).starts_with("[blast] seeds=2 unresolved=0 "));

    // Backward, the marker names the walk and the whole closure.
    let out = glia(&[
        "blast-radius", &api, "app::save", "app::publish", "--direction", "backward", "--depth", "6",
        "--json",
    ]);
    assert!(out.status.success());
    assert_eq!(marker(&out), "[blast] seeds=2 unresolved=0 reached=2 linked_seeds=0 walk=Backward");
}

#[test]
fn the_table_names_each_rows_seed_and_the_unresolved_queries() {
    let repo = TempRepo::new("table");
    let api = repo.api();
    let out = glia(&["blast-radius", &api, "app::create_order", "app::save", "nope", "--direction", "forward"]);
    assert!(out.status.success(), "a mixed answer exits 0: {:?}", out.status);
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("> unresolved: nope"), "{table}");
    assert!(table.contains("> seed `app::create_order` is one carry edge from: app::save"), "{table}");
    assert!(table.contains("| seed |"), "a seed column with more than one seed:\n{table}");
    assert!(table.contains("`app::audit` | `app::save` |"), "audit is save's:\n{table}");
    assert_eq!(marker(&out), "[blast] seeds=2 unresolved=1 reached=3 linked_seeds=1 walk=Forward");

    // One seed: no seed column, as before LD.5.
    let out = glia(&["blast-radius", &api, "app::save"]);
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("| score | depth | live | via | kind | qname | location |"), "{table}");
    assert!(!table.contains("| seed |"), "{table}");
}

#[test]
fn every_query_unknown_exits_3_with_the_absence() {
    let repo = TempRepo::new("unknown");
    let api = repo.api();
    let out = glia(&["blast-radius", &api, "nope", "zilch"]);
    assert_eq!(out.status.code(), Some(3));
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("> unresolved: nope, zilch"), "{table}");
    assert!(table.contains("> FACT: no node has the qname or name `nope`"), "{table}");
    let out = glia(&["blast-radius", &api, "nope", "--json"]);
    assert_eq!(out.status.code(), Some(3), "the JSON answer exits 3 too");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(v["absence"]["reason"], "unknown_symbol", "{v}");
    assert_eq!(v["results"], serde_json::json!([]), "{v}");
}

/// DD.10: the three ops of one contract class (identity-contract-ops: the
/// swagger op, the swaggo op and the feature.yaml op, DERIVED) are one
/// DOC_SECTION row with its `identity` by default and three rows with
/// `--no-collapse`. The `[identity] collapse surface=blast_radius` stderr line
/// is DD.10's fired_on marker, printed beside the unchanged `[blast]` line.
#[test]
fn identity_classes_collapse_unless_no_collapse() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../bench/substrate-gap/fixtures/identity-contract-ops");
    let args = ["blast-radius", fixture, "GET /api/orders", "--direction", "backward", "--depth", "1"];
    let docs = |v: &serde_json::Value| -> Vec<serde_json::Value> {
        v["results"].as_array().expect("results").iter().filter(|r| r["kind"] == "DOC_SECTION").cloned().collect()
    };

    let out = glia(&[&args[..], &["--json"]].concat());
    assert!(out.status.success(), "exit {:?}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let rows = docs(&v);
    assert_eq!(rows.len(), 1, "one row for the class: {v}");
    let id = &rows[0]["identity"];
    assert_eq!(id["tier"], "derived", "{v}");
    assert_eq!(id["rules"], serde_json::json!(["contract_op", "contract_route"]), "{v}");
    assert_eq!(id["also_in"].as_array().map(Vec::len), Some(2), "{v}");
    assert_eq!(marker(&out), "[blast] seeds=1 unresolved=0 reached=1 linked_seeds=0 walk=Backward");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().filter(|l| l.starts_with("[identity] collapse surface=blast_radius")).collect();
    assert_eq!(lines.len(), 1, "one identity line per answer:\n{stderr}");
    eprintln!("{}", lines[0]);
    assert!(lines[0].ends_with(" folded=0 rows=1"), "{}", lines[0]);

    let out = glia(&[&args[..], &["--json", "--no-collapse"]].concat());
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let rows = docs(&v);
    assert_eq!(rows.len(), 3, "one row per op: {v}");
    assert!(rows.iter().all(|r| r.get("identity").is_none()), "{v}");
    assert_eq!(marker(&out), "[blast] seeds=1 unresolved=0 reached=3 linked_seeds=0 walk=Backward");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("[identity] collapse"), "no class, no identity line");

    // The table's location cell names the other places.
    let out = glia(&args);
    assert!(out.status.success());
    let table = String::from_utf8_lossy(&out.stdout);
    let row = table.lines().find(|l| l.contains("| DOC_SECTION |")).expect("the class row");
    assert!(row.contains(" (also in: "), "{table}");
    assert_eq!(table.lines().filter(|l| l.contains("| DOC_SECTION |")).count(), 1, "{table}");
}

// ---- EA.8: `--ranking`, `--no-cochange`, `--top-files` and `## files` ----
//
// EA.5's tree (engine/tests/rank_blast.rs): `svc/core.py` defines `core`;
// `svc/a.py`, `b.py`, `c.py` each call it once, `svc/d.py` twice; `web/page.ts`
// is unrelated code. The history snapshot: core changed with c 5 times, with
// page 4 times, with b twice and alone 5 times (C0: c 5/17, page 4/17, b 2/17).

mod ranked {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::sync::Arc;

    use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};
    use glia_engine::rank::{RankBase, RankCache, Ranking, rank_files};

    const T0: i64 = 1_767_225_600;
    const CORE: &str = "svc/core.py";
    const B: &str = "svc/b.py";
    const C: &str = "svc/c.py";
    const PAGE: &str = "web/page.ts";

    /// EA.5's HEAD (0.5.2) backward depth-1 rows from `core` over a tree
    /// directory named `tree` (the repo id, so the node-id ties, follow it).
    const HEAD_ROWS: [&str; 5] = ["svc::a::a", "svc::d::d2", "svc::b::b", "svc::d::d1", "svc::c::c"];

    fn sources() -> Vec<(&'static str, &'static str)> {
        vec![
            (CORE, "def core(x):\n    return x\n"),
            ("svc/a.py", "from svc.core import core\n\n\ndef a():\n    return core(1)\n"),
            (B, "from svc.core import core\n\n\ndef b():\n    return core(1)\n"),
            (C, "from svc.core import core\n\n\ndef c():\n    return core(1)\n"),
            (
                "svc/d.py",
                "from svc.core import core\n\n\ndef d1():\n    return core(1)\n\n\ndef d2():\n    return core(2)\n",
            ),
            (PAGE, "export function renderPage(t: string): string {\n  return t;\n}\n"),
        ]
    }

    fn history() -> Vec<HistoryCommit> {
        let groups: Vec<(Vec<&str>, usize)> =
            vec![(vec![CORE, C], 5), (vec![CORE, PAGE], 4), (vec![CORE, B], 2), (vec![CORE], 5)];
        let mut out = Vec::new();
        let mut i = 0usize;
        for (paths, n) in groups {
            for _ in 0..n {
                i += 1;
                out.push(HistoryCommit {
                    c: format!("{i:04}{}", "0".repeat(36)),
                    t: T0 + i64::try_from(i).expect("small") * 3_600,
                    files: paths
                        .iter()
                        .map(|p| HistoryFile { p: (*p).to_string(), a: Some(1), d: Some(0), from: None })
                        .collect(),
                });
            }
        }
        out.reverse();
        out
    }

    /// `<tmp>/glia-ea8-<tag>-<pid>/tree`, with the snapshot when asked;
    /// removed on drop.
    pub(super) struct Tree(PathBuf);

    impl Tree {
        pub(super) fn new(tag: &str, snapshot: bool) -> Self {
            let base = std::env::temp_dir().join(format!("glia-ea8-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let root = base.join("tree");
            for (p, text) in sources() {
                let path = root.join(p);
                std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
                std::fs::write(path, text).expect("write");
            }
            if snapshot {
                let h = history();
                let head = h.first().map_or_else(|| "0".repeat(40), |c| c.c.clone());
                write_history(&root, HistoryMeta::new(head, 2000, None, String::new()), &h, &[])
                    .expect("write snapshot");
            }
            Tree(base)
        }

        pub(super) fn root(&self) -> PathBuf {
            self.0.join("tree")
        }

        pub(super) fn path(&self) -> String {
            self.root().to_str().expect("utf-8").to_string()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(super) fn glia(args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glia")).env("GLIA_NO_PERSIST", "1").args(args).output().expect("glia")
    }

    fn json(out: &Output) -> serde_json::Value {
        assert!(out.status.success(), "exit {:?}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).expect("stdout is JSON")
    }

    fn rows(v: &serde_json::Value) -> Vec<String> {
        v["results"].as_array().expect("results").iter().map(|r| r["qname"].as_str().expect("q").to_string()).collect()
    }

    fn files(v: &serde_json::Value) -> Vec<String> {
        v["files"].as_array().expect("files").iter().map(|f| f["file"].as_str().expect("f").to_string()).collect()
    }

    /// The one line of stderr starting with `prefix`, relayed for grep.
    pub(super) fn line(out: &Output, prefix: &str) -> String {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let ls: Vec<&str> = stderr.lines().filter(|l| l.starts_with(prefix)).collect();
        assert!(!ls.is_empty(), "no `{prefix}` line:\n{stderr}");
        eprintln!("{}", ls[0]);
        ls[0].to_string()
    }

    fn blast(t: &Tree, extra: &[&str]) -> Output {
        let p = t.path();
        let mut a = vec!["blast-radius", p.as_str(), "core", "--direction", "backward", "--depth", "1"];
        a.extend_from_slice(extra);
        glia(&a)
    }

    pub(super) fn path_of(root: &Path) -> String {
        root.to_str().expect("utf-8").to_string()
    }

    #[test]
    fn default_ranks_cochanged_files_first() {
        let t = Tree::new("h10", true);
        let out = blast(&t, &["--json"]);
        let v = json(&out);
        let r = rows(&v);
        assert_eq!(r, ["svc::c::c", "svc::b::b", "svc::a::a", "svc::d::d2", "svc::d::d1"], "{v}");
        assert_eq!(v["results"][0]["file"], C, "{v}");
        assert_eq!(files(&v)[0], C, "{v}");
        assert_eq!(&files(&v)[..3], [C, PAGE, B], "{v}");
        assert!(line(&out, "[rank-cache] holder=").starts_with("[rank-cache] holder=cli "));
        assert!(line(&out, "[rank] surface=blast_radius").contains(" ranking=H10 "));
        let cochange = line(&out, "[rank] surface=blast_radius");
        assert!(!cochange.contains(" cochange_files=0 "), "{cochange}");
    }

    #[test]
    fn no_cochange_is_the_head_order() {
        let t = Tree::new("p1", true);
        let v = json(&blast(&t, &["--no-cochange", "--json"]));
        assert_eq!(rows(&v), HEAD_ROWS, "{v}");
        let bare = Tree::new("p1-bare", false);
        let w = json(&blast(&bare, &["--json"]));
        assert_eq!(rows(&w), HEAD_ROWS, "no snapshot: the default is the head order: {w}");
    }

    #[test]
    fn ppr_wide_lists_files_in_pt_star_order() {
        let t = Tree::new("ptstar", true);
        let out = blast(&t, &["--ranking", "ppr-wide", "--no-cochange", "--json"]);
        let v = json(&out);
        assert!(line(&out, "[rank] surface=blast_radius").contains(" ranking=PT* "));

        let built = glia_engine::generate_one(&path_of(&t.root())).expect("build");
        let cache = Arc::new(RankCache::new("test", &built.repo_roots, &built.repo_labels));
        let fi = cache.files(&built.merged);
        let seed = fi.index_of(fi.repo(0), CORE).expect("core.py is a candidate");
        let wide = Ranking::new(RankBase::PprWide, false);
        let fr = rank_files(&built.merged, &cache, &[seed], &[], wide);
        let mut per: BTreeMap<u32, usize> = BTreeMap::new();
        for r in v["results"].as_array().expect("results") {
            let f = fi.file_of_node(glia_core::NodeId(r["id"].as_u64().expect("id"))).expect("candidate");
            *per.entry(f).or_default() += 1;
        }
        let want: Vec<String> = fr.top(&fi, 10, &per).iter().map(|f| f.file.clone()).collect();
        assert_eq!(files(&v), want, "{v}");
    }

    #[test]
    fn bfs_marker_and_markdown_table() {
        let t = Tree::new("bfs", true);
        let out = blast(&t, &["--ranking", "bfs"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(line(&out, "[rank] surface=blast_radius").contains(" ranking=H10[B0] "));
        let md = String::from_utf8_lossy(&out.stdout);
        assert!(md.contains("\n## files\n\n| # | file | rows | co-change | depth |\n"), "{md}");
        assert!(md.contains(&format!("| 1 | {C} | 1 | 0.294 | 1 |")), "{md}");

        let ppr = blast(&t, &[]);
        let md = String::from_utf8_lossy(&ppr.stdout);
        assert!(md.contains("\n## files\n\n| # | file | rows | co-change | ppr |\n"), "{md}");
    }

    #[test]
    fn top_files_cuts_the_list_and_a_bad_ranking_is_clap() {
        let t = Tree::new("top", true);
        let v = json(&blast(&t, &["--top-files", "3", "--json"]));
        assert_eq!(files(&v).len(), 3, "{v}");
        let bad = blast(&t, &["--ranking", "x"]);
        assert_eq!(bad.status.code(), Some(2), "{}", String::from_utf8_lossy(&bad.stderr));
        assert!(String::from_utf8_lossy(&bad.stderr).contains("[possible values: ppr, ppr-wide, bfs]"));
    }
}
