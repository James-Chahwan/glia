//! LG.3c — `glia flows --features / --out`: the feature-flow files the
//! dogfood repos' agents read, written by the real binary over a scratch copy
//! of `tests/fixtures/flows_stack` (LG.3a's two repos: the Angular `web`
//! calling `GET /api/orders`, and the Go `api` serving it and subscribing to
//! `orders.created`). Every test copies the fixture first, so a write the
//! engine should have refused lands in the scratch dir, never in the tree.
//!
//! The CLI's `[feature-flows] wrote <w> feature file(s), <u> unchanged, <r>
//! removed -> <dir>` stderr line is the fired_on marker; asserting it here
//! makes it a tested contract. Grep it from a run:
//! `glia flows <repo> --out <dir> 2>&1 >/dev/null | grep '^\[feature-flows\] wrote [0-9]'`.
//!
//! Without `--features` / `--out`, `glia flows` is LD.4b's per-entry report,
//! byte for byte: the first test pins its JSON to the engine's `entry_flows`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// A scratch copy of `tests/fixtures/flows_stack` (`web/`, `api/`) plus an
/// `out/` path, removed on drop (`cli` has no dev-dependencies, so no
/// `tempfile`).
struct Scratch {
    root: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("scratch dir");
    for e in std::fs::read_dir(from).expect("read fixture dir") {
        let e = e.expect("fixture dir entry");
        let target = to.join(e.file_name());
        if e.file_type().expect("file type").is_dir() {
            copy_tree(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), &target).expect("copy fixture file");
        }
    }
}

/// Every file under `dir` with its bytes, by path relative to `dir`.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).expect("under base").to_path_buf();
                out.insert(rel, std::fs::read(&p).expect("read file"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-lg3c-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/flows_stack");
        copy_tree(&fixture, &root);
        Scratch { root }
    }

    fn web(&self) -> String {
        self.root.join("web").to_string_lossy().into_owned()
    }

    fn api(&self) -> String {
        self.root.join("api").to_string_lossy().into_owned()
    }

    fn out(&self) -> PathBuf {
        self.root.join("out")
    }

    /// `glia flows <web> --with <api> <args>`, no layout persisted.
    fn flows(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glia"))
            .arg("flows")
            .arg(self.web())
            .arg("--with")
            .arg(self.api())
            .args(args)
            .env("GLIA_NO_PERSIST", "1")
            .output()
            .expect("run glia")
    }

    /// `glia flows ... --out <scratch>/out <args>`.
    fn flows_out(&self, args: &[&str]) -> Output {
        let out = self.out();
        let mut all = vec!["--out", out.to_str().expect("utf-8 scratch path")];
        all.extend_from_slice(args);
        self.flows(&all)
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The `[feature-flows] wrote <w> feature file(s), ...` line of a CLI write.
fn wrote_line(o: &Output) -> String {
    stderr(o)
        .lines()
        .find(|l| {
            l.strip_prefix("[feature-flows] wrote ")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
        .unwrap_or_else(|| panic!("no CLI `[feature-flows] wrote N ...` line:\n{}", stderr(o)))
        .to_string()
}

fn index(dir: &Path) -> Value {
    let text = std::fs::read_to_string(dir.join("index.json")).expect("index.json written");
    serde_json::from_str(&text).expect("index.json is JSON")
}

fn index_features(dir: &Path) -> Vec<String> {
    index(dir)["features"]
        .as_array()
        .expect("features list")
        .iter()
        .map(|r| r["feature"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn flows_without_feature_flags_is_ld4b_unchanged() {
    let s = Scratch::new("ld4b");
    let out = s.flows(&["--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let built = glia_engine::generate_many(&[s.web(), s.api()]).expect("build");
    let want = glia_engine::trace::entry_flows(
        &built.merged,
        &built.repo_labels,
        glia_engine::trace::DEFAULT_DEPTH,
    );
    assert_eq!(
        stdout(&out),
        format!("{}\n", serde_json::to_string(&want).expect("serialises"))
    );
    assert!(
        !stderr(&out).contains("[feature-flows]"),
        "{}",
        stderr(&out)
    );

    // The table too: LD.4b's header, no feature-flow output.
    let table = s.flows(&[]);
    assert_eq!(table.status.code(), Some(0), "{}", stderr(&table));
    assert!(
        stdout(&table).contains("| key | kind | entry | reach | xsvc | mechanisms | location |"),
        "{}",
        stdout(&table)
    );
    assert!(
        !stderr(&table).contains("[feature-flows]"),
        "{}",
        stderr(&table)
    );
    assert!(
        !s.root.join("web/.glia").exists(),
        "the report writes nothing"
    );
}

#[test]
fn flows_writes_index_and_feature_files() {
    let s = Scratch::new("write");
    let out = s.flows_out(&[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let dir = s.out();
    assert_eq!(
        stdout(&out),
        format!("{}\n", dir.join("index.json").display())
    );
    let features = index_features(&dir);
    for key in ["orders", "queue-orders.created"] {
        assert!(
            features.iter().any(|f| f == key),
            "no `{key}` in {features:?}"
        );
    }
    let orders = std::fs::read_to_string(dir.join("orders.yaml")).expect("orders.yaml");
    let first = orders.lines().next().unwrap_or("");
    assert!(first.starts_with("# generated by glia"), "{first}");
    assert!(
        orders
            .lines()
            .any(|l| l.contains("\"via\":\"HTTP_CALLS\"") && l.contains("\"cross_service\":true")),
        "{orders}"
    );
    let n = features.len();
    assert_eq!(
        wrote_line(&out),
        format!(
            "[feature-flows] wrote {n} feature file(s), 0 unchanged, 0 removed -> {}",
            dir.display()
        )
    );
    assert!(
        !s.root.join("web/.glia").exists(),
        "--out writes only there"
    );
}

#[test]
fn flows_features_json_prints_without_writing() {
    let s = Scratch::new("json");
    let before = snapshot(&s.root);
    let out = s.flows(&["--features", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_str(&stdout(&out)).expect("stdout is JSON");
    let records = v.as_array().expect("a JSON array");
    assert!(
        records.iter().any(|r| r["feature"] == "orders"),
        "{}",
        stdout(&out)
    );
    assert_eq!(snapshot(&s.root), before, "--json writes nothing");
    assert!(
        !stderr(&out).contains("[feature-flows] wrote"),
        "{}",
        stderr(&out)
    );

    // `--feature` keeps one record; `--group-by entry` keys by entry.
    let one = s.flows(&["--features", "--json", "--feature", "orders"]);
    let v: Value = serde_json::from_str(&stdout(&one)).expect("stdout is JSON");
    let keys: Vec<&str> = v
        .as_array()
        .expect("a JSON array")
        .iter()
        .filter_map(|r| r["feature"].as_str())
        .collect();
    assert_eq!(keys, ["orders"]);
    let by_entry = s.flows(&["--features", "--json", "--group-by", "entry"]);
    assert_eq!(by_entry.status.code(), Some(0), "{}", stderr(&by_entry));
    let v: Value = serde_json::from_str(&stdout(&by_entry)).expect("stdout is JSON");
    assert!(
        v.as_array()
            .expect("a JSON array")
            .iter()
            .any(|r| r["feature"] == "get_-api-orders"),
        "{}",
        stdout(&by_entry)
    );
}

#[test]
fn features_without_out_writes_the_default_dir() {
    let s = Scratch::new("default");
    let out = s.flows(&["--features"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let dir = glia_engine::feature_flows::default_flows_dir(Path::new(&s.web()));
    assert_eq!(
        stdout(&out),
        format!("{}\n", dir.join("index.json").display())
    );
    assert!(index_features(&dir).iter().any(|f| f == "orders"));
    assert!(wrote_line(&out).ends_with(&format!("-> {}", dir.display())));
}

#[test]
fn second_run_is_unchanged() {
    let s = Scratch::new("rerun");
    let first = s.flows_out(&[]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let bytes = snapshot(&s.out());
    let n = index_features(&s.out()).len();
    let second = s.flows_out(&[]);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    assert_eq!(
        wrote_line(&second),
        format!(
            "[feature-flows] wrote 0 feature file(s), {n} unchanged, 0 removed -> {}",
            s.out().display()
        )
    );
    assert_eq!(snapshot(&s.out()), bytes);
}

#[test]
fn zero_features_is_an_empty_index() {
    let s = Scratch::new("empty");
    let out = s.flows_out(&["--feature", "no-such-feature"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(index_features(&s.out()).is_empty());
    assert!(wrote_line(&out).starts_with("[feature-flows] wrote 0 feature file(s), 0 unchanged"));
}

#[test]
fn bad_group_by_exits_2() {
    let s = Scratch::new("groupby");
    let out = s.flows_out(&["--group-by", "service"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("feature") && err.contains("entry"), "{err}");
    assert!(!s.out().exists());
}

#[test]
fn out_dir_inside_walked_tree_exits_2() {
    let s = Scratch::new("walked");
    let inside = s.root.join("web/docs/flows");
    let out = s.flows(&["--out", inside.to_str().expect("utf-8")]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("refusing"), "{}", stderr(&out));
    assert!(!s.root.join("web/docs").exists(), "nothing written");
}

#[test]
fn feature_flags_need_features_or_out() {
    let s = Scratch::new("flags");
    for args in [
        &["--scope", "web"][..],
        &["--feature", "orders"][..],
        &["--group-by", "entry"][..],
    ] {
        let out = s.flows(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
    let both = s.flows_out(&["--json"]);
    assert_eq!(both.status.code(), Some(2), "{}", stderr(&both));
    assert!(!s.out().exists());
}

/// GB.1: `--concept <word>` is the engine's `concept_flows` (the flows that
/// touch the word), exit 0 with rows, 1 on the `no_match` absence, 2 beside
/// `--features` / `--out`. The `[concept] word=.. surface=cli` line is the
/// fired_on marker.
#[test]
fn concept_lists_the_flows_that_touch_a_word() {
    let s = Scratch::new("concept");
    let out = s.flows(&["--concept", "orders", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let built = glia_engine::generate_many(&[s.web(), s.api()]).expect("build");
    let mut opts = glia_engine::trace::ConceptOptions::default();
    opts.collapse = glia_engine::identity::Collapse::with_labels(&built.repo_labels);
    let want =
        glia_engine::trace::concept_flows(&built.merged, &built.repo_labels, "orders", &opts);
    assert!(!want.results.is_empty());
    assert_eq!(
        stdout(&out),
        format!("{}\n", serde_json::to_string(&want).expect("serialises"))
    );
    let marker = stderr(&out)
        .lines()
        .find(|l| l.starts_with("[concept] word=orders "))
        .unwrap_or_else(|| panic!("no [concept] line:\n{}", stderr(&out)))
        .to_string();
    assert!(marker.contains(" surface=cli "), "{marker}");
    assert!(
        marker.contains(&format!(" hits={} ", want.results.len())) || want.results.len() == 20,
        "{marker}"
    );

    // The table: one row per hit, the matched hop shown.
    let table = s.flows(&["--concept", "orders"]);
    assert_eq!(table.status.code(), Some(0), "{}", stderr(&table));
    let text = stdout(&table);
    assert!(
        text.contains("| # | key | entry | tier | matched | hop | depth | location |"),
        "{text}"
    );

    // Nothing touches the word: the absence, exit 1.
    let none = s.flows(&["--concept", "zebra", "--json"]);
    assert_eq!(none.status.code(), Some(1), "{}", stderr(&none));
    let v: Value = serde_json::from_str(&stdout(&none)).expect("json");
    assert_eq!(v["results"], Value::Array(Vec::new()));
    assert_eq!(v["absence"]["reason"], "no_match");
    assert_eq!(v["absence"]["tier"], "fact");
    let none_table = s.flows(&["--concept", "zebra"]);
    assert_eq!(none_table.status.code(), Some(1));
    assert!(
        stdout(&none_table).contains("_(no flow touches `zebra`)_"),
        "{}",
        stdout(&none_table)
    );

    // Not with the feature records.
    let features = s.flows(&["--concept", "orders", "--features"]);
    assert_eq!(features.status.code(), Some(2), "{}", stderr(&features));
    let both = s.flows_out(&["--concept", "orders"]);
    assert_eq!(both.status.code(), Some(2), "{}", stderr(&both));
    assert!(!s.out().exists());
}

/// A one-repo Python tree for GB.3: `main` (shop/cli.py) -> `format_report`
/// -> `export_ledger_rows` (shop/ledger.py, documented as writing the ledger
/// out), and `test_warm` -> `warm_cache`. Removed on drop.
struct LedgerTree(PathBuf);

impl LedgerTree {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-gb3-cli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, src) in [
            ("pyproject.toml", "[project]\nname = \"shop\"\n"),
            ("shop/__init__.py", ""),
            (
                "shop/ledger.py",
                "def export_ledger_rows(rows):\n    \"\"\"Export the ledger rows to a csv file on disk.\"\"\"\n    out = []\n    for r in rows:\n        out.append(r)\n    return out\n",
            ),
            (
                "shop/report.py",
                "from shop.ledger import export_ledger_rows\n\n\ndef format_report(rows):\n    return export_ledger_rows(rows)\n",
            ),
            ("shop/cache.py", "def warm_cache():\n    return 1\n"),
            (
                "shop/cli.py",
                "from shop.report import format_report\n\n\ndef main():\n    return format_report([])\n",
            ),
            (
                "tests/test_warm.py",
                "from shop.cache import warm_cache\n\n\ndef test_warm():\n    assert warm_cache() == 1\n",
            ),
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("dir");
            std::fs::write(p, src).expect("write");
        }
        LedgerTree(root)
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("utf-8 temp path")
    }

    /// `glia <args>` with the test model as the surface model, in memory.
    fn glia(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glia"))
            .args(args)
            .env("GLIA_NO_PERSIST", "1")
            .env("GLIA_EMBED_MODEL", "test-hash")
            .output()
            .expect("glia runs")
    }

    /// `glia embed <tree> --model test-hash`, asserted.
    fn embed(&self) {
        let out = self.glia(&["embed", self.path(), "--model", "test-hash"]);
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert!(self.0.join(".glia/vectors.jsonl").is_file());
    }
}

impl Drop for LedgerTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// GB.3: a word no lexical tier matches is the `no_match` absence (exit 1)
/// without vectors; after `glia embed` it answers at tier `semantic`, the
/// flow through the top fused file first, with the `[flows] semantic` marker,
/// the table's lexical-miss line and its similarity column; `--no-semantic`
/// is the absence again, and a lexical match never changes.
#[test]
fn concept_falls_back_to_the_fused_semantic_tier() {
    let t = LedgerTree::new("flows");
    let q = "write the ledger rows out to a csv file";
    let before = t.glia(&["flows", t.path(), "--concept", q]);
    assert_eq!(before.status.code(), Some(1), "{}", stderr(&before));
    assert!(!stderr(&before).contains("[flows] semantic"));
    let lexical = t.glia(&["flows", t.path(), "--concept", "ledger", "--json"]);
    assert_eq!(lexical.status.code(), Some(0), "{}", stderr(&lexical));

    t.embed();
    let after = t.glia(&["flows", t.path(), "--concept", q, "--json"]);
    assert_eq!(after.status.code(), Some(0), "{}", stderr(&after));
    let marker = stderr(&after)
        .lines()
        .find(|l| l.starts_with("[flows] semantic query="))
        .unwrap_or_else(|| panic!("no [flows] semantic line:\n{}", stderr(&after)))
        .to_string();
    eprintln!("{marker}");
    assert!(
        marker.starts_with(&format!("[flows] semantic query={q} files="))
            && marker.contains(" top_file_rank=1 "),
        "{marker}"
    );
    let v: Value = serde_json::from_str(&stdout(&after)).expect("json");
    assert!(v["absence"].is_null(), "{v}");
    let top = &v["results"][0];
    assert_eq!(top["tier"], "semantic", "{v}");
    assert_eq!(top["key"], "main", "{v}");
    assert_eq!(top["hop"]["to_file"], "shop/ledger.py", "{v}");
    assert!(top["similarity"].is_f64(), "{v}");

    let table = t.glia(&["flows", t.path(), "--concept", q]);
    assert_eq!(table.status.code(), Some(0), "{}", stderr(&table));
    let text = stdout(&table);
    assert!(
        text.contains("ranked by the fused file order: heuristic)_")
            && text.contains(
                "| # | key | entry | tier | matched | hop | depth | similarity | location |"
            ),
        "{text}"
    );

    let off = t.glia(&["flows", t.path(), "--concept", q, "--no-semantic"]);
    assert_eq!(off.status.code(), Some(1), "{}", stderr(&off));
    assert_eq!(stdout(&off), stdout(&before));
    assert!(!stderr(&off).contains("[flows] semantic"));
    let lexical_after = t.glia(&["flows", t.path(), "--concept", "ledger", "--json"]);
    assert_eq!(stdout(&lexical_after), stdout(&lexical));

    // --no-semantic needs --concept.
    let bare = t.glia(&["flows", t.path(), "--no-semantic"]);
    assert_eq!(bare.status.code(), Some(2), "{}", stderr(&bare));
}
