//! CD.2b — `splits` (global mode): the module or community quotient of a
//! scope, bisected by Stoer-Wagner at the ratio-best phase cut above a balance
//! floor, recursive to N parts, cut edges located at their evidence sites and
//! each part diffed against `glia arch`.
//!
//! One Python repo, three packages. `orders/` and `billing/` hold four
//! modules each; a module defines two functions, and each function calls both
//! functions of every other module in its package, imported by name
//! (`from orders.cart import add_item, total`), so each package is one dense
//! cluster at the node level too (the community quotient's view). As built,
//! two modules of one package are joined by 8 CALLS and 4 IMPORTS (one
//! IMPORTS per imported name): weight 8 * 4 + 4 * 1 = 36. The packages are
//! joined by exactly two
//! calls, `orders.api.checkout -> billing.charge.charge` and
//! `billing.charge.refund -> orders.repo.reopen`, each with its IMPORTS: the
//! seam weighs 2 * (4 + 1) = 10. `util/fmt.py` (the module and `money`) is
//! called once, from `orders.cart.total`: its unit is joined by one CALLS and
//! one IMPORTS, 5, below the seam, so the global minimum cut peels the leaf
//! and the balanced cut is the seam, by construction. The three `__init__.py`
//! files are empty modules with no weighted edge, so they stay out: 9 units.
//!
//! The fired_on marker is read from a child process: `child_splits` re-runs
//! this test binary on one tree with `--nocapture` and the parent reads its
//! stderr (the `communities.rs` way).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

use glia_code_domain::{edge_category, node_kind};
use glia_core::NodeId;
use glia_engine::generate_one;
use glia_engine::profile::CODE_PROFILE;
use glia_engine::splits::{SplitAnswer, SplitArgs, SplitPart, splits};
use glia_graph::MergedGraph;

/// Set on the child run: the tree `child_splits` builds.
const CHILD_ENV: &str = "GLIA_CD2B_CHILD_DIR";

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

/// The source of `<pkg>/<module>.py`: its imports (sorted by module, names
/// sorted), then its two functions. `prefix` is the dotted package root
/// (`app.`) or empty.
fn module_py(pkg: &str, package: &Package, module: &str, prefix: &str) -> String {
    let fns = package
        .iter()
        .find(|(m, _)| *m == module)
        .map(|(_, f)| *f)
        .expect("a module of the package");
    let mut imports: BTreeMap<(String, String), BTreeSet<&str>> = BTreeMap::new();
    let mut bodies: Vec<Vec<&str>> = Vec::new();
    for f in fns.iter() {
        let mut body = Vec::new();
        for (other, ofns) in package.iter().filter(|(m, _)| *m != module) {
            imports
                .entry((pkg.to_string(), other.to_string()))
                .or_default()
                .extend(ofns);
            body.extend(ofns);
        }
        for ((fp, fm, ff), (tp, tm, tf)) in CROSSING {
            if (fp, fm, ff) == (pkg, module, *f) {
                imports
                    .entry((tp.to_string(), tm.to_string()))
                    .or_default()
                    .insert(tf);
                body.push(tf);
            }
        }
        bodies.push(body);
    }
    let mut src = String::new();
    for ((p, m), names) in &imports {
        let names: Vec<&str> = names.iter().copied().collect();
        src.push_str(&format!(
            "from {prefix}{p}.{m} import {}\n",
            names.join(", ")
        ));
    }
    for (f, body) in fns.iter().zip(&bodies) {
        src.push_str(&format!("\n\ndef {f}():\n"));
        for callee in body {
            src.push_str(&format!("    {callee}()\n"));
        }
    }
    src
}

/// Every file of the fixture, repo-relative, under `dir` (`app/` or empty).
fn sources(dir: &str) -> Vec<(String, String)> {
    let prefix = dir.trim_end_matches('/').replace('/', ".");
    let prefix = if prefix.is_empty() {
        prefix
    } else {
        format!("{prefix}.")
    };
    let mut files = Vec::new();
    for (pkg, package) in [("orders", &ORDERS), ("billing", &BILLING)] {
        files.push((format!("{dir}{pkg}/__init__.py"), String::new()));
        for (m, _) in package.iter() {
            files.push((
                format!("{dir}{pkg}/{m}.py"),
                module_py(pkg, package, m, &prefix),
            ));
        }
    }
    files.push((format!("{dir}util/__init__.py"), String::new()));
    files.push((
        format!("{dir}util/fmt.py"),
        "def money():\n    return 0\n".to_string(),
    ));
    files
}

fn write_fixture(root: &Path, dir: &str) {
    for (rel, src) in sources(dir) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
}

fn build(root: &Path) -> (MergedGraph, BTreeMap<u64, String>) {
    let r = generate_one(root.to_str().expect("utf-8 temp path")).expect("generate_one");
    (r.merged, r.repo_labels)
}

/// The fixture under `<tempdir>/fixture`, its packages under `dir`.
fn fixture_in(dir: &str) -> (tempfile::TempDir, MergedGraph, BTreeMap<u64, String>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("fixture");
    write_fixture(&root, dir);
    let (m, labels) = build(&root);
    (tmp, m, labels)
}

fn fixture() -> (tempfile::TempDir, MergedGraph, BTreeMap<u64, String>) {
    fixture_in("")
}

fn run(
    m: &MergedGraph,
    labels: &BTreeMap<u64, String>,
    set: impl FnOnce(&mut SplitArgs),
) -> SplitAnswer {
    let mut args = SplitArgs::default();
    set(&mut args);
    splits(m, labels, &args)
}

/// The 1-based line of the call to `callee` inside `def <caller>():` of
/// `src`.
fn call_line(src: &str, caller: &str, callee: &str) -> i64 {
    let lines: Vec<&str> = src.lines().collect();
    let def = lines
        .iter()
        .position(|l| *l == format!("def {caller}():"))
        .expect("the caller is defined");
    let at = lines[def..]
        .iter()
        .position(|l| *l == format!("    {callee}()"))
        .expect("the caller calls the callee");
    (def + at) as i64 + 1
}

fn source_of(rel: &str) -> String {
    sources("")
        .into_iter()
        .find(|(r, _)| r == rel)
        .map(|(_, s)| s)
        .expect("a fixture file")
}

fn modules_of(pkg: &str, package: &Package, dir: &str) -> Vec<String> {
    let prefix = dir.trim_end_matches('/').replace('/', "::");
    let prefix = if prefix.is_empty() {
        prefix
    } else {
        format!("{prefix}::")
    };
    package
        .iter()
        .map(|(m, _)| format!("{prefix}{pkg}::{m}"))
        .collect()
}

fn part_with<'a>(a: &'a SplitAnswer, module: &str) -> &'a SplitPart {
    a.parts
        .iter()
        .find(|p| p.modules.iter().any(|m| m == module))
        .unwrap_or_else(|| panic!("no part holds {module}: {a:#?}"))
}

/// Each node's part, by its enclosing MODULE's label in a part's `modules`:
/// the answer's grouping recomputed from the graph.
fn node_parts(m: &MergedGraph, a: &SplitAnswer) -> HashMap<NodeId, u32> {
    let part_of_module: HashMap<&str, u32> = a
        .parts
        .iter()
        .flat_map(|p| p.modules.iter().map(move |q| (q.as_str(), p.id)))
        .collect();
    let mut out = HashMap::new();
    for g in &m.graphs {
        for n in &g.nodes {
            let mut cur = n.id;
            for _ in 0..64 {
                if g.nav.kind_by_id.get(&cur) == Some(&node_kind::MODULE) {
                    let q = g.nav.qname_by_id.get(&cur).map_or("", String::as_str);
                    if let Some(&p) = part_of_module.get(q) {
                        out.entry(n.id).or_insert(p);
                    }
                    break;
                }
                match g.nav.parent_of.get(&cur) {
                    Some(&p) => cur = p,
                    None => break,
                }
            }
        }
    }
    out
}

/// `(summed community weight, edges)` of every graph edge between two parts.
fn crossing(m: &MergedGraph, a: &SplitAnswer) -> (u64, usize) {
    let part = node_parts(m, a);
    let (mut weight, mut edges) = (0u64, 0usize);
    for e in m.all_edges() {
        let w = u64::from(CODE_PROFILE.tables.community_weight(e.category));
        if w == 0 {
            continue;
        }
        if let (Some(pa), Some(pb)) = (part.get(&e.from), part.get(&e.to))
            && pa != pb
        {
            weight += w;
            edges += 1;
        }
    }
    (weight, edges)
}

#[test]
fn bisects_along_the_seam() {
    let (_tmp, m, labels) = fixture();
    let a = run(&m, &labels, |_| {});
    assert!(a.absence.is_none(), "{:?}", a.absence);
    assert_eq!(
        (a.mode, a.quotient, a.tier),
        ("global", "module", "heuristic")
    );
    assert_eq!(
        a.units, 9,
        "8 package modules + util::fmt; the __init__ modules have no weighted edge"
    );
    assert_eq!(a.parts.len(), 2, "{a:#?}");

    let mut orders_side = modules_of("orders", &ORDERS, "");
    orders_side.push("util::fmt".to_string());
    let billing_side = modules_of("billing", &BILLING, "");
    let po = part_with(&a, "orders::api");
    let pb = part_with(&a, "billing::charge");
    assert_eq!(po.modules, orders_side);
    assert_eq!(pb.modules, billing_side);
    assert_eq!((po.units, pb.units), (5, 4));
    assert_eq!(
        (po.nodes, pb.nodes),
        (14, 12),
        "a module and its two functions each; util::fmt and money"
    );
    assert_eq!((po.id, pb.id), (0, 1), "numbered by nodes descending");
    assert_eq!(
        (po.label.as_str(), pb.label.as_str()),
        ("orders", "billing")
    );
    assert_eq!(po.entries + pb.entries, 0, "no entrypoint in the fixture");
    assert!(!po.top_members.is_empty() && po.top_members.len() <= 10);

    // The cut weight, recomputed from the graph rather than pinned.
    let (weight, edges) = crossing(&m, &a);
    assert!(weight > 0);
    assert_eq!(a.cut_weight, weight);
    assert_eq!(a.cut_edges_total, edges);
    assert_eq!(a.cut_edges.len(), edges, "under the default cap");
    assert!(
        a.global_min_weight < a.cut_weight,
        "the util/ leaf is the global minimum: {} vs {}",
        a.global_min_weight,
        a.cut_weight
    );
    assert!(a.balanced);

    // Both seam calls, located at their call sites, heaviest first.
    let calls: Vec<_> = a
        .cut_edges
        .iter()
        .filter(|c| c.category == "CALLS")
        .collect();
    assert_eq!(calls.len(), 2, "{:#?}", a.cut_edges);
    assert!(a.cut_edges[..2].iter().all(|c| c.category == "CALLS"));
    let want = [
        (
            "orders::api::checkout",
            "billing::charge::charge",
            "orders/api.py",
            "checkout",
            "charge",
        ),
        (
            "billing::charge::refund",
            "orders::repo::reopen",
            "billing/charge.py",
            "refund",
            "reopen",
        ),
    ];
    for (from, to, file, caller, callee) in want {
        let c = calls
            .iter()
            .find(|c| c.from_qname == from && c.to_qname == to)
            .unwrap_or_else(|| panic!("no cut edge {from} -> {to}: {:#?}", a.cut_edges));
        assert_eq!(c.file.as_deref(), Some(file));
        assert_eq!(
            c.line,
            Some(call_line(&source_of(file), caller, callee)),
            "{c:?}"
        );
        assert_eq!(c.basis, Some("site"));
        assert_eq!(
            c.weight,
            CODE_PROFILE.tables.community_weight(edge_category::CALLS)
        );
        assert_ne!(c.from_part, c.to_part);
    }
    assert_eq!(
        (
            calls[0].from_qname.as_str(),
            calls[0].from_part,
            calls[0].to_part
        ),
        ("billing::charge::refund", pb.id, po.id),
        "equal weight and category: from qname order"
    );
}

#[test]
fn arch_diff() {
    let (_tmp, m, labels) = fixture();
    let a = run(&m, &labels, |_| {});
    let po = part_with(&a, "orders::api");
    let pb = part_with(&a, "billing::charge");
    let diff = |id: u32| {
        a.arch
            .iter()
            .find(|d| d.part == id)
            .expect("a diff per part")
    };
    assert_eq!(a.arch.len(), 2);
    assert_eq!(diff(pb.id).verdict, "aligned");
    assert_eq!(diff(pb.id).services, ["billing"]);
    assert_eq!(diff(po.id).verdict, "spans_services");
    assert_eq!(diff(po.id).services, ["orders", "util"], "count descending");
    assert_eq!(
        po.services,
        [("orders".to_string(), 12), ("util".to_string(), 2)]
    );
    assert_eq!(pb.services, [("billing".to_string(), 12)]);

    // Under one top-level dir both parts cut inside the one service.
    let (_tmp2, m2, labels2) = fixture_in("app/");
    let b = run(&m2, &labels2, |_| {});
    assert!(b.absence.is_none(), "{:?}", b.absence);
    let qo = part_with(&b, "app::orders::api");
    let qb = part_with(&b, "app::billing::charge");
    let mut orders_side = modules_of("orders", &ORDERS, "app/");
    orders_side.push("app::util::fmt".to_string());
    assert_eq!(qo.modules, orders_side, "the same bisection");
    assert_eq!(qb.modules, modules_of("billing", &BILLING, "app/"));
    assert_eq!(b.arch.len(), 2);
    for d in &b.arch {
        assert_eq!(d.verdict, "splits_service", "{d:?}");
        assert_eq!(d.services, ["app"]);
    }
}

#[test]
fn three_parts() {
    let (_tmp, m, labels) = fixture();
    let two = run(&m, &labels, |_| {});
    let a = run(&m, &labels, |a| a.parts = 3);
    assert!(a.absence.is_none(), "{:?}", a.absence);
    assert_eq!(a.parts.len(), 3, "{a:#?}");
    for (i, p) in a.parts.iter().enumerate() {
        assert_eq!(p.id, i as u32);
        assert!(p.nodes > 0 && p.units > 0, "never an empty part: {p:?}");
    }
    let nodes: Vec<usize> = a.parts.iter().map(|p| p.nodes).collect();
    assert!(
        nodes.windows(2).all(|w| w[0] >= w[1]),
        "numbered by nodes descending: {nodes:?}"
    );
    assert_eq!(a.parts.iter().map(|p| p.units).sum::<usize>(), a.units);

    // The larger side (orders/ + util/, 14 nodes) is the one split again;
    // billing/ stays whole.
    let billing = modules_of("billing", &BILLING, "");
    assert!(a.parts.iter().any(|p| p.modules == billing), "{a:#?}");
    let larger = part_with(&two, "orders::api");
    let mut carved: Vec<String> = a
        .parts
        .iter()
        .filter(|p| p.modules != billing)
        .flat_map(|p| p.modules.clone())
        .collect();
    carved.sort();
    assert_eq!(carved, larger.modules);
    let (weight, edges) = crossing(&m, &a);
    assert_eq!((a.cut_weight, a.cut_edges_total), (weight, edges));
    assert!(a.cut_weight > two.cut_weight);

    // More parts than the cut can make useful still never yields an empty
    // part, and never more parts than units.
    let many = run(&m, &labels, |a| a.parts = 8);
    assert_eq!(many.parts.len(), 8);
    assert!(many.parts.iter().all(|p| p.nodes > 0 && p.units > 0));
    let over = run(&m, &labels, |a| a.parts = 50);
    assert_eq!(over.parts.len(), 8, "parts is clamped to 2..=8");
}

#[test]
fn community_quotient() {
    let (_tmp, m, labels) = fixture();
    let by_module = run(&m, &labels, |_| {});
    let by_community = run(&m, &labels, |a| a.quotient = "community".to_string());
    assert!(by_community.absence.is_none(), "{:?}", by_community.absence);
    assert_eq!(by_community.quotient, "community");
    assert!(by_community.units >= 2);
    assert_eq!(by_community.parts.len(), 2);
    for (p, q) in by_module.parts.iter().zip(&by_community.parts) {
        assert_eq!(
            (p.id, p.nodes, &p.modules, &p.services),
            (q.id, q.nodes, &q.modules, &q.services)
        );
    }
    assert_eq!(
        by_community.cut_weight, by_module.cut_weight,
        "the same seam"
    );
    assert_eq!(by_community.cut_edges_total, by_module.cut_edges_total);
}

#[test]
fn deterministic() {
    let (_t1, m1, l1) = fixture();
    let (_t2, m2, l2) = fixture();
    for quotient in ["module", "community"] {
        for parts in [2, 3] {
            let set = |a: &mut SplitArgs| {
                a.quotient = quotient.to_string();
                a.parts = parts;
            };
            let json = |m: &MergedGraph, l: &BTreeMap<u64, String>| {
                serde_json::to_string(&run(m, l, set)).expect("serialise")
            };
            let first = json(&m1, &l1);
            assert_eq!(first, json(&m1, &l1), "{quotient} x{parts}: two calls");
            assert_eq!(first, json(&m2, &l2), "{quotient} x{parts}: two builds");
        }
    }
}

#[test]
fn empty_scope_absence() {
    let (_tmp, m, labels) = fixture();
    let one = run(&m, &labels, |a| a.scope = Some("util".to_string()));
    assert_eq!(one.units, 1);
    assert!(one.parts.is_empty() && one.cut_edges.is_empty() && one.arch.is_empty());
    let why = one.absence.as_ref().expect("one unit is an absence");
    assert_eq!(why.reason, "no_match");
    assert!(
        why.note.starts_with("fewer than two units in scope"),
        "{}",
        why.note
    );

    let none = run(&m, &labels, |a| a.scope = Some("nowhere".to_string()));
    assert_eq!(none.units, 0);
    assert_eq!(none.absence.as_ref().map(|a| a.reason), Some("no_match"));

    let scoped = run(&m, &labels, |a| a.scope = Some("orders".to_string()));
    assert!(scoped.absence.is_none(), "{:?}", scoped.absence);
    assert_eq!(scoped.units, 4, "orders/ alone");

    let refused = run(&m, &labels, |a| a.quotient = "files".to_string());
    assert_eq!(refused.quotient, "none");
    assert!(refused.parts.is_empty());
    assert_eq!(refused.absence.as_ref().map(|a| a.reason), Some("no_match"));
}

/// Child half of `marker_line`: runs the answer on the tree in [`CHILD_ENV`]
/// and prints it; a no-op in a normal test run.
#[test]
fn child_splits() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        let (m, labels) = build(Path::new(&dir));
        let a = splits(&m, &labels, &SplitArgs::default());
        println!("{}", serde_json::to_string(&a).expect("serialise"));
    }
}

#[test]
fn marker_line() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fixture(tmp.path(), "");
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_splits", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, tmp.path())
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    // libtest prints `test child_splits ... ` before the child's own line.
    let json = stdout
        .lines()
        .find_map(|l| l.find("{\"mode\"").map(|i| &l[i..]))
        .unwrap_or_else(|| panic!("no answer on the child's stdout: {stdout}"));
    let a: serde_json::Value = serde_json::from_str(json).expect("the answer is JSON");
    let want = format!(
        "[splits] mode=global quotient=module units={} parts={} cut_weight={} global_min={} balanced={} cut_edges={} surface=engine",
        a["units"],
        a["parts"].as_array().map_or(0, Vec::len),
        a["cut_weight"],
        a["global_min_weight"],
        a["balanced"],
        a["cut_edges_total"],
    );
    assert!(
        stderr.lines().any(|l| l == want),
        "no `{want}` on the child's stderr:\n{stderr}"
    );
}
