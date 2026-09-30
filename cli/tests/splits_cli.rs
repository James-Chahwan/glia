//! CD.2d — `glia splits`, driving the real binary over CD.2b / CD.2c's
//! fixture (engine/tests/splits.rs, rebuilt here by the same generator): one
//! Python repo, three packages. `orders/` and `billing/` hold four modules
//! each; a module defines two functions, and each function calls both
//! functions of every other module in its package, imported by name, so each
//! package is one dense cluster. The packages are joined by exactly two calls,
//! `orders.api.checkout -> billing.charge.charge` and
//! `billing.charge.refund -> orders.repo.reopen` (the seam, weight 10), and
//! `util/fmt.py` hangs off `orders.cart.total` (weight 5, the global
//! minimum). `orders.repo.save` INSERTs into the sqlite table `orders` and
//! `billing.ledger.post` UPDATEs it: one shared write. The seam calls run both
//! ways: one cycle between the two parts.
//!
//! The engine's `[splits] mode=<m> ... surface=cli` stderr line is the
//! fired_on marker; asserting it here makes it a tested contract.
//!
//! `thread_count_invariant`: the rayon pool is sized once per process, so it
//! runs the binary twice as child processes (`GLIA_THREADS=1`, then unset) and
//! compares the `--json` bytes (the `parallel_cli.rs` way).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

type Package = [(&'static str, [&'static str; 2]); 4];

const ORDERS: Package = [
    ("api", ["create", "checkout"]),
    ("cart", ["add_item", "total"]),
    ("repo", ["save", "reopen"]),
    ("stock", ["reserve", "release"]),
];
const BILLING: Package = [
    ("charge", ["charge", "refund"]),
    ("invoice", ["issue", "void"]),
    ("ledger", ["post", "balance"]),
    ("report", ["summary", "export"]),
];

type Site = (&'static str, &'static str, &'static str);

/// The calls that leave a package: `(package, module, function)` calls
/// `(package, module, function)`.
const CROSSING: [(Site, Site); 3] = [
    (
        ("orders", "api", "checkout"),
        ("billing", "charge", "charge"),
    ),
    (
        ("billing", "charge", "refund"),
        ("orders", "repo", "reopen"),
    ),
    (("orders", "cart", "total"), ("util", "fmt", "money")),
];

/// The statement a function runs after its calls: `(package, module,
/// function, SQL)`, as `conn.execute("<SQL>")`.
const SQL: [(&str, &str, &str, &str); 3] = [
    (
        "orders",
        "repo",
        "save",
        "INSERT INTO orders (id) VALUES (1)",
    ),
    (
        "billing",
        "ledger",
        "post",
        "UPDATE orders SET paid = 1 WHERE id = 1",
    ),
    ("billing", "report", "summary", "SELECT * FROM ledger"),
];

/// The source of `<pkg>/<module>.py`: its imports (sorted by module, names
/// sorted), then its two functions (CD.2b's `module_py`).
fn module_py(pkg: &str, package: &Package, module: &str) -> String {
    let fns = package
        .iter()
        .find(|(m, _)| *m == module)
        .map(|(_, f)| *f)
        .expect("a module of the package");
    let mut imports: BTreeMap<(String, String), BTreeSet<&str>> = BTreeMap::new();
    let mut bodies: Vec<Vec<String>> = Vec::new();
    for f in fns.iter() {
        let mut body = Vec::new();
        for (other, ofns) in package.iter().filter(|(m, _)| *m != module) {
            imports
                .entry((pkg.to_string(), other.to_string()))
                .or_default()
                .extend(ofns);
            body.extend(ofns.iter().map(|c| format!("    {c}()")));
        }
        for &((fp, fm, ff), (tp, tm, tf)) in &CROSSING {
            if (fp, fm, ff) == (pkg, module, *f) {
                imports
                    .entry((tp.to_string(), tm.to_string()))
                    .or_default()
                    .insert(tf);
                body.push(format!("    {tf}()"));
            }
        }
        for (sp, sm, sf, sql) in SQL {
            if (sp, sm, sf) == (pkg, module, *f) {
                body.push(format!("    conn.execute(\"{sql}\")"));
            }
        }
        bodies.push(body);
    }
    let mut src = String::new();
    for ((p, m), names) in &imports {
        let names: Vec<&str> = names.iter().copied().collect();
        src.push_str(&format!("from {p}.{m} import {}\n", names.join(", ")));
    }
    for (f, body) in fns.iter().zip(&bodies) {
        src.push_str(&format!("\n\ndef {f}():\n"));
        for line in body {
            src.push_str(line);
            src.push('\n');
        }
    }
    src
}

/// Every file of the fixture, repo-relative.
fn sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    for (pkg, package) in [("orders", &ORDERS), ("billing", &BILLING)] {
        files.push((format!("{pkg}/__init__.py"), String::new()));
        for (m, _) in package.iter() {
            files.push((format!("{pkg}/{m}.py"), module_py(pkg, package, m)));
        }
    }
    files.push(("util/__init__.py".to_string(), String::new()));
    files.push((
        "util/fmt.py".to_string(),
        "def money():\n    return 0\n".to_string(),
    ));
    files
}

fn source_of(rel: &str) -> String {
    sources()
        .into_iter()
        .find(|(r, _)| r == rel)
        .map(|(_, s)| s)
        .expect("a fixture file")
}

/// The 1-based line of the call to `callee` inside `def <caller>():` of
/// `src`.
fn call_line(src: &str, caller: &str, callee: &str) -> usize {
    let lines: Vec<&str> = src.lines().collect();
    let def = lines
        .iter()
        .position(|l| *l == format!("def {caller}():"))
        .expect("the caller is defined");
    let at = lines[def..]
        .iter()
        .position(|l| *l == format!("    {callee}()"))
        .expect("the caller calls the callee");
    def + at + 1
}

/// The 1-based line of `def <name>():` in `src`.
fn def_line(src: &str, name: &str) -> usize {
    src.lines()
        .position(|l| l == format!("def {name}():"))
        .expect("the function is defined")
        + 1
}

/// A fresh temp root holding the fixture, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("glia-cd2d-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        for (rel, src) in sources() {
            let path = p.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, src).expect("write source");
        }
        Root(p)
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("utf-8 temp path")
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `glia splits <args>` with `GLIA_THREADS` set to `threads`, or unset.
fn run(args: &[&str], threads: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
    cmd.arg("splits").args(args).env("GLIA_NO_PERSIST", "1");
    match threads {
        Some(t) => cmd.env("GLIA_THREADS", t),
        None => cmd.env_remove("GLIA_THREADS"),
    };
    cmd.output().expect("glia runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Run `glia splits <args>`, relaying the fired_on marker, and check the exit
/// code.
fn glia(args: &[&str], want: i32) -> Output {
    let out = run(args, None);
    assert_eq!(
        out.status.code(),
        Some(want),
        "glia splits {args:?}\nstdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );
    // Relay the marker so `-- --nocapture | grep '^\[splits\] '` sees it.
    for line in markers(&out) {
        eprintln!("{line}");
    }
    out
}

fn markers(out: &Output) -> Vec<String> {
    stderr(out)
        .lines()
        .filter(|l| l.starts_with("[splits] "))
        .map(str::to_string)
        .collect()
}

/// The non-empty lines of the `## <heading>` section, up to the next one.
fn section<'a>(text: &'a str, heading: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|l| *l != heading)
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| !l.is_empty())
        .collect()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("--json parses")
}

#[test]
fn seam_fixture() {
    let root = Root::new("seam");
    let out = glia(&[root.path()], 0);
    let text = stdout(&out);

    let m = markers(&out);
    assert_eq!(m.len(), 1, "one marker per answer: {m:?}");
    assert!(
        m[0].starts_with("[splits] mode=global quotient=module units=9 parts=2 "),
        "{}",
        m[0]
    );
    for part in [" balanced=true ", " shared_writes=1 ", " part_cycles=1 "] {
        assert!(m[0].contains(part), "{part} in {}", m[0]);
    }
    assert!(m[0].ends_with(" surface=cli"), "{}", m[0]);

    let heading = text
        .lines()
        .find(|l| l.starts_with("Suggested cut (heuristic): "))
        .unwrap_or_else(|| panic!("a heading:\n{text}"));
    assert!(
        heading.starts_with("Suggested cut (heuristic): global, module quotient, cut weight ")
            && heading.ends_with(", balanced yes)"),
        "{heading}"
    );
    assert!(heading.contains(" (global min "), "{heading}");

    // The parts table: orders (with util/) first by nodes, then billing.
    let parts = section(&text, "## Parts");
    assert_eq!(
        parts[..4],
        [
            "| part | label | nodes | units | services | entries |",
            "|--:|---|--:|--:|---|--:|",
            "| 0 | `orders` | 14 | 5 | orders 12, util 2 | 0 |",
            "| 1 | `billing` | 12 | 4 | billing 12 | 0 |",
        ],
        "{text}"
    );
    assert!(
        parts.contains(
            &"- part 1 modules: `billing::charge`, `billing::invoice`, `billing::ledger`, `billing::report`"
        ),
        "{text}"
    );

    // Both seam calls, located 1-based at their call sites, heaviest first.
    let edges = section(&text, "## Cut edges");
    let checkout = format!(
        "| `orders::api::checkout` -> `billing::charge::charge` | 0 -> 1 | CALLS | 4 | orders/api.py:{} |",
        call_line(&source_of("orders/api.py"), "checkout", "charge")
    );
    let refund = format!(
        "| `billing::charge::refund` -> `orders::repo::reopen` | 1 -> 0 | CALLS | 4 | billing/charge.py:{} |",
        call_line(&source_of("billing/charge.py"), "refund", "reopen")
    );
    assert_eq!(
        edges[..4],
        [
            "| from -> to | parts | category | weight | at |",
            "|---|---|---|--:|---|",
            refund.as_str(),
            checkout.as_str(),
        ],
        "the two CALLS first (weight, then from qname):\n{text}"
    );
    assert!(
        edges[4..].iter().all(|l| l.contains(" | IMPORTS | 1 | ")),
        "then their IMPORTS:\n{text}"
    );

    // The blockers.
    let writes = section(&text, "## Shared writes");
    let orders = format!(
        "| `data_entity:sql:orders` | DATA_ENTITY | 0, 1 | 0 write, 1 write | `orders::repo::save` orders/repo.py:{}; `billing::ledger::post` billing/ledger.py:{} | derived |",
        def_line(&source_of("orders/repo.py"), "save"),
        def_line(&source_of("billing/ledger.py"), "post"),
    );
    assert_eq!(
        writes,
        [
            "| entity | kind | parts | modes | writers | tier |",
            "|---|---|---|---|---|---|",
            orders.as_str(),
        ],
        "both writers, by part, located at their declarations:\n{text}"
    );
    let cycles = section(&text, "## Cycles between parts");
    assert_eq!(cycles.len(), 3, "{text}");
    assert!(
        cycles[2].starts_with("| 0, 1 | 0 -> 1 `")
            && cycles[2].contains("; 1 -> 0 `")
            && cycles[2].ends_with(" | derived |"),
        "{}",
        cycles[2]
    );

    assert_eq!(
        section(&text, "## Against glia arch"),
        [
            "- part 0: spans_services orders, util",
            "- part 1: aligned billing"
        ],
        "{text}"
    );

    // The anchored mode, by paths.
    let out = glia(&[root.path(), "--from", "orders", "--to", "billing"], 0);
    let text = stdout(&out);
    assert!(
        text.lines()
            .any(|l| l.starts_with("Suggested cut (heuristic): st, module quotient, cut weight ")),
        "{text}"
    );
    let m = markers(&out);
    assert!(
        m.len() == 1 && m[0].starts_with("[splits] mode=st ") && m[0].ends_with(" surface=cli"),
        "{m:?}"
    );
    assert!(
        section(&text, "## Parts").contains(&"| 1 | `billing` | 12 | 4 | billing 12 | 0 |"),
        "part 1 is the sink side:\n{text}"
    );

    // --parts outside 2..=8 is refused by clap, before any build.
    for bad in ["9", "1"] {
        let out = glia(&[root.path(), "--parts", bad], 2);
        assert!(markers(&out).is_empty(), "refused before any build");
        assert!(stderr(&out).contains("--parts"), "{}", stderr(&out));
    }
}

#[test]
fn json_is_the_engine_answer() {
    let root = Root::new("json");
    let out = glia(&[root.path(), "--json"], 0);
    let raw = stdout(&out);
    assert!(
        raw.starts_with("{\"mode\":\"global\",\"quotient\":\"module\",\"units\":9,\"parts\":["),
        "engine field order: {raw}"
    );
    let v = json(&out);
    let keys: Vec<&str> = v
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    let mut want = [
        "mode",
        "quotient",
        "units",
        "parts",
        "cut_weight",
        "global_min_weight",
        "balanced",
        "cut_edges_total",
        "cut_edges",
        "arch",
        "shared_writes",
        "cycles",
        "tier",
        "absence",
    ];
    let mut got = keys.clone();
    got.sort_unstable();
    want.sort_unstable();
    assert_eq!(got, want, "{v}");
    assert!(v["absence"].is_null(), "{v}");
    assert_eq!(v["parts"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        v["shared_writes"][0]["modes"],
        serde_json::json!([[0, "write"], [1, "write"]])
    );

    // Options reach the engine.
    let out = glia(
        &[
            root.path(),
            "--parts",
            "3",
            "--quotient",
            "community",
            "--seed",
            "7",
            "--min-share",
            "0.2",
            "--json",
        ],
        0,
    );
    let v = json(&out);
    assert_eq!(v["quotient"], "community", "{v}");
    let m = markers(&out);
    assert!(
        m.len() == 1 && m[0].starts_with("[splits] mode=global quotient=community "),
        "{m:?}"
    );
    let v = json(&glia(&[root.path(), "--scope", "orders", "--json"], 0));
    assert_eq!(v["units"], 4, "orders/ alone: {v}");
}

#[test]
fn refused_arguments_exit_2() {
    let root = Root::new("refused");
    for (args, want) in [
        (
            vec!["--from", "orders"],
            "error: --from needs --to: the sink is not set",
        ),
        (
            vec!["--to", "billing"],
            "error: --to needs --from: the source is not set",
        ),
        (
            vec!["--min-share", "0.9"],
            "error: --min-share must be a number in [0, 0.5], got 0.9",
        ),
        (
            vec!["--min-share=-0.1"],
            "error: --min-share must be a number in [0, 0.5], got -0.1",
        ),
        (
            vec!["--min-share", "NaN"],
            "error: --min-share must be a number in [0, 0.5], got NaN",
        ),
    ] {
        let mut all = vec![root.path()];
        all.extend(args.iter().copied());
        let out = glia(&all, 2);
        assert!(stderr(&out).contains(want), "{args:?}: {}", stderr(&out));
        assert!(
            markers(&out).is_empty(),
            "{args:?}: refused before any build"
        );
        assert!(stdout(&out).is_empty(), "{args:?}");
    }
    // clap refuses a quotient the engine does not build.
    let out = glia(&[root.path(), "--quotient", "files"], 2);
    assert!(markers(&out).is_empty(), "refused before any build");
}

#[test]
fn no_cut_is_an_absence() {
    let root = Root::new("absence");
    let out = glia(&[root.path(), "--scope", "util"], 1);
    let text = stdout(&out);
    assert!(
        text.contains("No cut: global, module quotient, 1 unit(s)"),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|l| l.starts_with("> FACT: fewer than two units in scope")),
        "the absence says why:\n{text}"
    );
    assert!(!text.contains("Suggested cut"), "{text}");

    let out = glia(
        &[
            root.path(),
            "--from",
            "orders::api",
            "--to",
            "orders::api::checkout",
            "--json",
        ],
        1,
    );
    let v = json(&out);
    assert_eq!(v["mode"], "st");
    assert_eq!(v["absence"]["reason"], "no_match", "{v}");
    assert!(
        v["absence"]["note"]
            .as_str()
            .is_some_and(|n| n.contains("share")),
        "{v}"
    );
    assert_eq!(v["parts"], serde_json::json!([]));
}

#[test]
fn build_error_exits_2() {
    let missing = Path::new(&std::env::temp_dir())
        .join(format!("glia-cd2d-cli-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    let out = glia(&[missing.to_str().expect("utf-8 temp path")], 2);
    assert!(markers(&out).is_empty());
}

#[test]
fn thread_count_invariant() {
    let root = Root::new("threads");
    let one = run(&[root.path(), "--json"], Some("1"));
    let pool = run(&[root.path(), "--json"], None);
    for (name, out) in [("GLIA_THREADS=1", &one), ("GLIA_THREADS unset", &pool)] {
        assert_eq!(out.status.code(), Some(0), "{name}: {}", stderr(out));
    }
    assert!(!one.stdout.is_empty());
    assert_eq!(
        stdout(&one),
        stdout(&pool),
        "the pool size never reaches the answer"
    );
}
