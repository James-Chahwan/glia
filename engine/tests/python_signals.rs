//! DF.13 acceptance: blinker and Django signals pair across files. A
//! signal's identity is declared in one file (`order_placed =
//! signal("order-placed")`, `order_placed = Signal()`) and used in another
//! (`from signals import order_placed`, then `order_placed.send(..)` /
//! `@order_placed.connect` / `@receiver(order_placed)`). The per-file
//! extractors are cached by the file's own hash and cannot read the declaring
//! file, so a post-cache graft (`engine/src/build/grafts.rs`
//! `apply_python_signals`) builds the repo's signal table, binds each file's
//! imported names and swaps the file's event nodes for ones that include its
//! signal sites.
//!
//! `grade.py` reads the installed wheel, so these tests build the sources from
//! the working tree. The `[event-py-signals]` fired_on marker is read from a
//! child process, as `event_const_names.rs` reads `[event-const]`.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::{GenerateResult, ParseCache, generate_one, generate_one_with_cache};
use glia_graph::MergedGraph;

/// matrix/python/eventbus: a named blinker signal.
const BLINKER_SIGNALS: &str = "from blinker import signal\n\norder_placed = signal(\"order-placed\")\n";

const BLINKER_ORDERS: &str = "from signals import order_placed\n\n\n@order_placed.connect\ndef send_receipt(sender, **kw):\n    print(\"receipt\", kw)\n\n\ndef place(order):\n    order_placed.send(\"orders\", order=order)\n";

/// matrix/python/eventbus_django: a Django signal, the variable its identity.
const DJANGO_SIGNALS: &str = "import django.dispatch\n\norder_placed = django.dispatch.Signal()\n";

const DJANGO_RECEIVERS: &str = "from django.dispatch import receiver\n\nfrom signals import order_placed\n\n\n@receiver(order_placed)\ndef send_receipt(sender, **kwargs):\n    print(\"receipt\")\n\n\ndef place(order):\n    order_placed.send(sender=None, order=order)\n";

/// A Django app's signal module.
const APP_SIGNALS: &str = "from django.dispatch import Signal\n\norder_placed = Signal()\n";

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

/// A temp repo holding `files`.
fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let td = tempfile::tempdir().unwrap();
    for (rel, text) in files {
        write(td.path(), rel, text);
    }
    td
}

fn build(dir: &Path) -> GenerateResult {
    generate_one(dir.to_str().unwrap()).expect("generate_one")
}

/// Every qname of `kind`, sorted and deduplicated across graphs.
fn qnames_of(m: &MergedGraph, kind: NodeKindId) -> Vec<String> {
    let set: BTreeSet<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter_map(move |n| {
                (g.nav.kind_by_id.get(&n.id) == Some(&kind))
                    .then(|| g.nav.qname_by_id.get(&n.id).cloned())
                    .flatten()
            })
        })
        .collect();
    set.into_iter().collect()
}

fn emitters(m: &MergedGraph) -> Vec<String> {
    qnames_of(m, node_kind::EVENT_EMITTER)
}

fn handlers(m: &MergedGraph) -> Vec<String> {
    qnames_of(m, node_kind::EVENT_HANDLER)
}

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// The id of the node named `q`.
fn id_of(m: &MergedGraph, q: &str) -> NodeId {
    m.graphs
        .iter()
        .find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .find(|(_, x)| x.as_str() == q)
                .map(|(id, _)| *id)
        })
        .unwrap_or_else(|| panic!("no node {q}"))
}

/// `(from qname, to qname)` for every edge of `category`, sorted.
fn edges(m: &MergedGraph, category: EdgeCategoryId) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = m
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (qname(m, e.from), qname(m, e.to)))
        .collect();
    out.sort();
    out
}

/// The cell kinds of node `q`, in order.
fn cell_kinds(m: &MergedGraph, q: &str) -> Vec<glia_core::CellTypeId> {
    let id = id_of(m, q);
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter().map(|c| c.kind))
        .collect()
}

/// The JSON payloads of node `q`'s cells of `kind`.
fn json_cells(m: &MergedGraph, q: &str, kind: glia_core::CellTypeId) -> Vec<String> {
    let id = id_of(m, q);
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == kind)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

/// The POSITION payloads of node `q`.
fn positions(m: &MergedGraph, q: &str) -> Vec<String> {
    json_cells(m, q, cell_type::POSITION)
}

/// The ORIGIN payloads of node `q`.
fn origins(m: &MergedGraph, q: &str) -> Vec<String> {
    json_cells(m, q, cell_type::ORIGIN)
}

fn s(a: &str) -> String {
    a.to_string()
}

fn pair(a: &str, b: &str) -> (String, String) {
    (s(a), s(b))
}

const CHILD_ENV: &str = "GLIA_DF13_CHILD_BUILD";

/// The child half of [`build_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it builds that tree so its stderr (with
/// `--nocapture`) carries the build's markers.
#[test]
fn child_build_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        build(Path::new(&dir));
    }
}

/// The `[event-py-signals]` lines of one `generate_one` over `dir`, without
/// their ` repo=<label>` tail.
fn signal_marker(dir: &Path) -> Vec<String> {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "--exact",
            "child_build_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, dir)
        .output()
        .expect("re-run the test binary");
    assert!(
        out.status.success(),
        "child build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("[event-py-signals]"))
        .map(|l| l.split(" repo=").next().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn blinker_signal_pairs_across_files() {
    let td = repo(&[("signals.py", BLINKER_SIGNALS), ("orders.py", BLINKER_ORDERS)]);
    let m = &build(td.path()).merged;
    assert_eq!(emitters(m), vec![s("event_emit:order-placed")]);
    assert_eq!(handlers(m), vec![s("event_handle:order-placed")]);
    assert_eq!(
        edges(m, edge_category::EVENT_FLOWS),
        vec![pair("event_emit:order-placed", "event_handle:order-placed")]
    );
    // The library's verb never names an event.
    assert!(!emitters(m).iter().any(|q| q == "event_emit:send"));

    // Each site is located and anchored to the function holding it.
    assert_eq!(
        positions(m, "event_emit:order-placed"),
        vec![s(r#"{"file":"orders.py","start_line":9,"end_line":9}"#)]
    );
    assert_eq!(
        edges(m, edge_category::USES)
            .into_iter()
            .filter(|(_, to)| to == "event_emit:order-placed")
            .collect::<Vec<_>>(),
        vec![pair("orders::place", "event_emit:order-placed")]
    );
    // A decorator site sits above its `def`, outside the function's span,
    // so the handler falls back to its module, as pyee's `@ee.on("x")` does.
    assert_eq!(
        edges(m, edge_category::CONTAINS)
            .into_iter()
            .filter(|(_, to)| to.starts_with("event_"))
            .collect::<Vec<_>>(),
        vec![pair("orders", "event_handle:order-placed")]
    );
    // Both anchor edges name the binding as their rule.
    let rules: Vec<_> = m
        .all_edges()
        .filter(|e| {
            e.to == id_of(m, "event_emit:order-placed")
                || e.to == id_of(m, "event_handle:order-placed")
        })
        .filter(|e| e.category != edge_category::EVENT_FLOWS)
        .map(|e| {
            let ev = Evidence::of(e).expect("an EVIDENCE cell");
            (ev.emitter, ev.rule)
        })
        .collect();
    assert_eq!(
        rules,
        vec![(s("extractor:anchor"), Some(s("python_signal"))); 2]
    );
    // The file's IMPORTS cell, like every other node of the parse, then the
    // synthetic-provenance ORIGIN every marker gets; no transport ORIGIN
    // (`delivery`): an in-process bus.
    for q in ["event_emit:order-placed", "event_handle:order-placed"] {
        assert_eq!(
            cell_kinds(m, q),
            vec![cell_type::POSITION, cell_type::IMPORTS, cell_type::ORIGIN],
            "{q}"
        );
        assert_eq!(origins(m, q), vec![s(r#"{"provenance":"synthetic"}"#)], "{q}");
    }
    assert_eq!(
        signal_marker(td.path()),
        vec![s("[event-py-signals] decls=1 files=1 emit=1 handle=1 dropped=0")]
    );
}

#[test]
fn django_signal_pairs_across_files() {
    let td = repo(&[("signals.py", DJANGO_SIGNALS), ("receivers.py", DJANGO_RECEIVERS)]);
    let m = &build(td.path()).merged;
    assert_eq!(emitters(m), vec![s("event_emit:signals.order_placed")]);
    assert_eq!(handlers(m), vec![s("event_handle:signals.order_placed")]);
    assert_eq!(
        edges(m, edge_category::EVENT_FLOWS),
        vec![pair(
            "event_emit:signals.order_placed",
            "event_handle:signals.order_placed"
        )]
    );
    assert_eq!(
        edges(m, edge_category::USES)
            .into_iter()
            .filter(|(_, to)| to.starts_with("event_"))
            .collect::<Vec<_>>(),
        vec![pair("receivers::place", "event_emit:signals.order_placed")]
    );
    assert_eq!(
        edges(m, edge_category::CONTAINS)
            .into_iter()
            .filter(|(_, to)| to.starts_with("event_"))
            .collect::<Vec<_>>(),
        vec![pair("receivers", "event_handle:signals.order_placed")]
    );
}

#[test]
fn relative_import_binds() {
    let td = repo(&[
        ("shop/__init__.py", ""),
        ("shop/signals.py", APP_SIGNALS),
        (
            "shop/orders.py",
            "from .signals import order_placed as placed\n\n\ndef place(order):\n    placed.send(sender=None)\n",
        ),
        (
            "shop/receivers.py",
            "from django.dispatch import receiver\n\nfrom .signals import order_placed\n\n\n@receiver([order_placed])\ndef on_placed(sender, **kw):\n    pass\n",
        ),
    ]);
    let m = &build(td.path()).merged;
    assert_eq!(emitters(m), vec![s("event_emit:shop.signals.order_placed")]);
    assert_eq!(handlers(m), vec![s("event_handle:shop.signals.order_placed")]);
    assert_eq!(
        edges(m, edge_category::EVENT_FLOWS),
        vec![pair(
            "event_emit:shop.signals.order_placed",
            "event_handle:shop.signals.order_placed"
        )]
    );
}

#[test]
fn module_imports_bind_the_dotted_name() {
    // `from . import signals` (a submodule), `import shop.signals` and
    // `import shop.signals as sg` each bind `<bound>.order_placed`.
    let td = repo(&[
        ("shop/__init__.py", ""),
        ("shop/signals.py", APP_SIGNALS),
        (
            "shop/orders.py",
            "from . import signals\n\n\ndef place(order):\n    signals.order_placed.send(sender=None)\n",
        ),
        (
            "shop/audit.py",
            "import shop.signals\n\n\ndef audit(order):\n    shop.signals.order_placed.send(sender=None)\n",
        ),
        (
            "shop/receivers.py",
            "import shop.signals as sg\n\n\ndef on_placed(sender, **kw):\n    pass\n\n\nsg.order_placed.connect(on_placed)\n",
        ),
    ]);
    let m = &build(td.path()).merged;
    assert_eq!(emitters(m), vec![s("event_emit:shop.signals.order_placed")]);
    assert_eq!(handlers(m), vec![s("event_handle:shop.signals.order_placed")]);
    let uses: Vec<_> = edges(m, edge_category::USES)
        .into_iter()
        .filter(|(_, to)| to.starts_with("event_emit:"))
        .collect();
    assert_eq!(
        uses,
        vec![
            pair("shop::audit::audit", "event_emit:shop.signals.order_placed"),
            pair("shop::orders::place", "event_emit:shop.signals.order_placed"),
        ]
    );
    assert_eq!(
        signal_marker(td.path()),
        vec![s("[event-py-signals] decls=1 files=3 emit=2 handle=1 dropped=0")]
    );
}

#[test]
fn package_init_declares_the_package_signal() {
    // A signal declared in `shop/__init__.py` is `shop.<name>`, imported as
    // `from shop import order_placed`.
    let td = repo(&[
        ("shop/__init__.py", APP_SIGNALS),
        (
            "orders.py",
            "from shop import order_placed\n\n\ndef place(order):\n    order_placed.send(sender=None)\n",
        ),
    ]);
    let m = &build(td.path()).merged;
    assert_eq!(emitters(m), vec![s("event_emit:shop.order_placed")]);
}

#[test]
fn two_apps_same_signal_name_do_not_pair() {
    // Two apps' `order_placed = Signal()` are two events: app_a sends its
    // own, app_b handles its own, and nothing joins them.
    let td = repo(&[
        ("app_a/__init__.py", ""),
        ("app_a/signals.py", APP_SIGNALS),
        (
            "app_a/orders.py",
            "from app_a.signals import order_placed\n\n\ndef place(order):\n    order_placed.send(sender=None)\n",
        ),
        ("app_b/__init__.py", ""),
        ("app_b/signals.py", APP_SIGNALS),
        (
            "app_b/receivers.py",
            "from django.dispatch import receiver\n\nfrom .signals import order_placed\n\n\n@receiver(order_placed)\ndef on_placed(sender, **kw):\n    pass\n",
        ),
    ]);
    let m = &build(td.path()).merged;
    assert_eq!(emitters(m), vec![s("event_emit:app_a.signals.order_placed")]);
    assert_eq!(handlers(m), vec![s("event_handle:app_b.signals.order_placed")]);
    assert!(edges(m, edge_category::EVENT_FLOWS).is_empty());
    assert_eq!(
        signal_marker(td.path()),
        vec![s("[event-py-signals] decls=2 files=2 emit=1 handle=1 dropped=0")]
    );
}

#[test]
fn unbound_send_mints_nothing() {
    // No import of a declared signal: `order_placed` here is any object with
    // a `.send(`. Importing a name the module does not declare binds nothing.
    let td = repo(&[
        ("signals.py", APP_SIGNALS),
        (
            "orders.py",
            "def place(order_placed):\n    order_placed.send(sender=None)\n",
        ),
        (
            "mail.py",
            "from signals import mailer\n\n\ndef notify():\n    mailer.send(\"hi\")\n",
        ),
    ]);
    let m = &build(td.path()).merged;
    assert!(emitters(m).is_empty(), "{:?}", emitters(m));
    assert!(handlers(m).is_empty(), "{:?}", handlers(m));
    assert_eq!(
        signal_marker(td.path()),
        vec![s("[event-py-signals] decls=1 files=0 emit=0 handle=0 dropped=0")]
    );
}

#[test]
fn a_name_declared_twice_differently_is_dropped() {
    let td = repo(&[
        (
            "signals.py",
            "from blinker import signal\n\norder_placed = signal(\"order-placed\")\norder_placed = signal(\"order-made\")\n",
        ),
        ("orders.py", BLINKER_ORDERS),
    ]);
    let m = &build(td.path()).merged;
    assert!(emitters(m).is_empty(), "{:?}", emitters(m));
    assert!(handlers(m).is_empty(), "{:?}", handlers(m));
    assert_eq!(
        signal_marker(td.path()),
        vec![s("[event-py-signals] decls=0 files=0 emit=0 handle=0 dropped=1")]
    );
}

#[test]
fn a_file_binding_one_name_to_two_signals_binds_neither() {
    // `order_placed` names app_a's signal, then app_b's: text alone cannot
    // say which one a site means, so neither binds.
    let td = repo(&[
        ("app_a/__init__.py", ""),
        ("app_a/signals.py", APP_SIGNALS),
        ("app_b/__init__.py", ""),
        ("app_b/signals.py", APP_SIGNALS),
        (
            "orders.py",
            "from app_a.signals import order_placed\nfrom app_b.signals import order_placed\n\n\ndef place():\n    order_placed.send(sender=None)\n",
        ),
    ]);
    let m = &build(td.path()).merged;
    assert!(emitters(m).is_empty(), "{:?}", emitters(m));
    assert_eq!(
        signal_marker(td.path()),
        vec![s("[event-py-signals] decls=2 files=0 emit=0 handle=0 dropped=1")]
    );
}

#[test]
fn same_file_declaration_binds() {
    let td = repo(&[(
        "signals.py",
        "from django.dispatch import Signal, receiver\n\norder_placed = Signal()\n\n\n@receiver(order_placed)\ndef on_placed(sender, **kw):\n    pass\n\n\ndef place():\n    order_placed.send(sender=None)\n",
    )]);
    let m = &build(td.path()).merged;
    assert_eq!(
        edges(m, edge_category::EVENT_FLOWS),
        vec![pair(
            "event_emit:signals.order_placed",
            "event_handle:signals.order_placed"
        )]
    );
}

#[test]
fn nested_project_root_resolves_absolute_imports() {
    // A Django project under `backend/` (its requirements.txt makes it a
    // python project root): `from orders.signals import ..` names
    // `backend/orders/signals.py`, the project root being on sys.path.
    let td = repo(&[
        ("backend/requirements.txt", "django==5.0\n"),
        ("backend/manage.py", "import sys\n"),
        ("backend/orders/__init__.py", ""),
        ("backend/orders/signals.py", APP_SIGNALS),
        (
            "backend/orders/views.py",
            "from orders.signals import order_placed\n\n\ndef place(order):\n    order_placed.send(sender=None)\n",
        ),
        (
            "backend/audit/receivers.py",
            "from django.dispatch import receiver\n\nfrom orders.signals import order_placed\n\n\n@receiver(order_placed)\ndef on_placed(sender, **kw):\n    pass\n",
        ),
    ]);
    let m = &build(td.path()).merged;
    let flows = edges(m, edge_category::EVENT_FLOWS);
    assert_eq!(flows.len(), 1, "{flows:?}");
    let (from, to) = &flows[0];
    assert!(
        from.starts_with("event_emit:backend.orders.signals.order_placed"),
        "{from}"
    );
    assert!(
        to.starts_with("event_handle:backend.orders.signals.order_placed"),
        "{to}"
    );
}

#[test]
fn signal_sites_keep_the_files_other_event_nodes() {
    // A file with a pyee literal site and a bound signal site: the pyee
    // nodes are re-emitted as they were, and a literal blinker site naming
    // the same event as a bound one mints one node.
    let td = repo(&[
        ("signals.py", BLINKER_SIGNALS),
        (
            "orders.py",
            "from blinker import signal\nfrom pyee import EventEmitter\n\nfrom signals import order_placed\n\nee = EventEmitter()\n\n\ndef place(order):\n    ee.emit(\"order-shipped\", order)\n    signal(\"order-placed\").send(\"orders\")\n    order_placed.send(\"orders\", order=order)\n",
        ),
    ]);
    let m = &build(td.path()).merged;
    assert_eq!(
        emitters(m),
        vec![s("event_emit:order-placed"), s("event_emit:order-shipped")]
    );
    let uses: Vec<_> = edges(m, edge_category::USES)
        .into_iter()
        .filter(|(_, to)| to.starts_with("event_emit:"))
        .collect();
    assert_eq!(
        uses,
        vec![
            pair("orders::place", "event_emit:order-placed"),
            pair("orders::place", "event_emit:order-shipped"),
        ]
    );
    // The edges the file held before keep the per-file pass's evidence: the
    // same file without the bound site gives the same (emitter, rule) pairs.
    let without = repo(&[
        ("signals.py", BLINKER_SIGNALS),
        (
            "orders.py",
            "from blinker import signal\nfrom pyee import EventEmitter\n\nee = EventEmitter()\n\n\ndef place(order):\n    ee.emit(\"order-shipped\", order)\n    signal(\"order-placed\").send(\"orders\")\n",
        ),
    ]);
    let plain = &build(without.path()).merged;
    let anchor_evidence = |m: &MergedGraph| -> Vec<(String, String, Option<String>)> {
        let mut out: Vec<_> = m
            .all_edges()
            .filter(|e| e.category == edge_category::USES)
            .filter(|e| qname(m, e.to).starts_with("event_emit:"))
            .map(|e| {
                let ev = Evidence::of(e).expect("an EVIDENCE cell");
                (qname(m, e.to), ev.emitter, ev.rule)
            })
            .collect();
        out.sort();
        out
    };
    assert_eq!(anchor_evidence(m), anchor_evidence(plain));
    assert!(
        anchor_evidence(m)
            .iter()
            .all(|(_, _, rule)| rule.as_deref() != Some("const_fold")),
        "{:?}",
        anchor_evidence(m)
    );
}

/// The `.gmap` shard bytes of one build, by file name.
fn gmap_bytes(r: &GenerateResult, out: &Path) -> Vec<(String, Vec<u8>)> {
    glia_store::write_merged_sharded(&r.merged, out).expect("write .gmap");
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(out)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

/// Every event node id, sorted.
fn event_ids(m: &MergedGraph) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav.kind_by_id.iter().filter_map(|(id, k)| {
                (*k == node_kind::EVENT_EMITTER || *k == node_kind::EVENT_HANDLER).then_some(*id)
            })
        })
        .collect();
    out.sort_by_key(|id| id.0);
    out
}

#[test]
fn cache_rebuild_is_identical() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("repo");
    write(&root, "signals.py", DJANGO_SIGNALS);
    write(&root, "receivers.py", DJANGO_RECEIVERS);
    let path = root.to_str().unwrap();

    let mut cache = ParseCache::new();
    let cold = generate_one_with_cache(path, &mut cache).expect("cold");
    assert_eq!(event_ids(&cold.merged).len(), 2);
    let warm = generate_one_with_cache(path, &mut cache).expect("warm");
    let diff = cache.last_diff().expect("a diff").clone();
    assert!(diff.reparsed.is_empty(), "{diff:?}");
    assert_eq!(event_ids(&warm.merged), event_ids(&cold.merged));
    // The cache keeps the pre-graft parse: no signal node in it.
    let cached = cache
        .iter()
        .find(|c| c.path == "receivers.py")
        .expect("cached parse");
    assert!(
        !cached
            .parse
            .nav
            .qname_by_id
            .values()
            .any(|q| q.starts_with("event_")),
        "the parse cache stores the pre-graft parse"
    );

    // Re-declaring the signal in its own file re-keys the reused importer.
    write(
        &root,
        "signals.py",
        "from blinker import signal\n\norder_placed = signal(\"order-placed\")\n",
    );
    let rekeyed = generate_one_with_cache(path, &mut cache).expect("rekeyed");
    let diff = cache.last_diff().expect("a diff").clone();
    assert!(diff.reused.contains(&s("receivers.py")), "{diff:?}");
    assert_eq!(emitters(&rekeyed.merged), vec![s("event_emit:order-placed")]);

    let clean = generate_one(path).expect("clean");
    assert_eq!(
        gmap_bytes(&rekeyed, &td.path().join("warm")),
        gmap_bytes(&clean, &td.path().join("clean")),
        "incremental == clean"
    );
}
