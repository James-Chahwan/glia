//! CB.3b acceptance: an event site keyed by a constant is keyed by the
//! constant's resolved literal through the repo const table, folded after the
//! parse cache like LA.4's queue topics, so `emit(OrderEvents.Created, ..)`
//! pairs with `@OnEvent("order.created")`. An ambiguous or unbound constant
//! keeps its path (CB.3a's fallback identity).
//!
//! Before it (CB.3a alone) the emitter was `event_emit:OrderEvents.Created`,
//! the handler `event_handle:order.created`, and nothing paired. `grade.py`
//! reads the installed wheel, so these tests build the sources from the
//! working tree. The `[event-const]` fired_on marker is read from a child
//! process: `child_build_for_stderr` re-runs this binary on one tree with
//! `--nocapture` and the parent reads its stderr (the
//! `channel_owner_rpc_event.rs` way).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::{GenerateResult, ParseCache, generate_one, generate_one_with_cache};
use glia_graph::MergedGraph;

const ORDERS_EVENTS: &str =
    "export const OrderEvents = { Created: \"order.created\", Paid: \"order.paid\" } as const;\n";

const ORDERS_SERVICE: &str = r#"import { Injectable } from "@nestjs/common";
import { EventEmitter2 } from "@nestjs/event-emitter";
import { OrderEvents } from "./orders.events";

@Injectable()
export class OrdersService {
  constructor(private eventEmitter: EventEmitter2) {}

  create(id: string) {
    this.eventEmitter.emit(OrderEvents.Created, { id });
  }
}
"#;

const AUDIT_LISTENER: &str = r#"import { Injectable } from "@nestjs/common";
import { OnEvent } from "@nestjs/event-emitter";

@Injectable()
export class AuditListener {
  @OnEvent("order.created")
  onCreated(payload: { id: string }) {}
}
"#;

const UNBOUND_SERVICE: &str = r#"import { Injectable } from "@nestjs/common";
import { EventEmitter2 } from "@nestjs/event-emitter";
import { Unbound } from "./external";

@Injectable()
export class UnboundService {
  constructor(private eventEmitter: EventEmitter2) {}

  fire() {
    this.eventEmitter.emit(Unbound.Thing, {});
  }
}
"#;

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

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
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

fn s(a: &str) -> String {
    a.to_string()
}

fn pair(a: &str, b: &str) -> (String, String) {
    (s(a), s(b))
}

const CHILD_ENV: &str = "GLIA_CB3B_CHILD_BUILD";

/// The child half of [`build_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it builds that tree so its stderr (with
/// `--nocapture`) carries the build's markers.
#[test]
fn child_build_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        build(Path::new(&dir));
    }
}

/// The stderr lines of one `generate_one` over `dir` that start with `prefix`.
fn build_stderr(dir: &Path, prefix: &str) -> Vec<String> {
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
        .filter(|l| l.starts_with(prefix))
        .map(String::from)
        .collect()
}

/// The marker line, without its ` repo=<label>` tail.
fn event_const_marker(dir: &Path) -> Vec<String> {
    build_stderr(dir, "[event-const]")
        .into_iter()
        .map(|l| l.split(" repo=").next().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn constant_folds_to_the_literal_the_other_side_names() {
    let td = repo(&[
        ("src/orders.events.ts", ORDERS_EVENTS),
        ("src/orders.service.ts", ORDERS_SERVICE),
        ("src/audit.listener.ts", AUDIT_LISTENER),
    ]);
    let r = build(td.path());
    let m = &r.merged;
    assert_eq!(
        qnames_of(m, node_kind::EVENT_EMITTER),
        vec![s("event_emit:order.created")]
    );
    assert_eq!(
        qnames_of(m, node_kind::EVENT_HANDLER),
        vec![s("event_handle:order.created")]
    );
    assert_eq!(
        edges(m, edge_category::EVENT_FLOWS),
        vec![pair(
            "event_emit:order.created",
            "event_handle:order.created"
        )]
    );
    // The folded emitter is located at its call site (0-indexed line 9) and
    // USED by the method holding it, re-anchored post-cache.
    let emit = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(move |n| (g, n)))
        .find(|(g, n)| {
            g.nav
                .qname_by_id
                .get(&n.id)
                .is_some_and(|q| q == "event_emit:order.created")
        })
        .map(|(_, n)| n.clone())
        .expect("folded emitter");
    let positions: Vec<String> = emit
        .cells
        .iter()
        .filter(|c| c.kind == cell_type::POSITION)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) => Some(j.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        positions,
        vec![s(
            r#"{"file":"src/orders.service.ts","start_line":9,"end_line":9}"#
        )]
    );
    let uses: Vec<_> = m
        .all_edges()
        .filter(|e| e.category == edge_category::USES && e.to == emit.id)
        .collect();
    assert_eq!(uses.len(), 1);
    assert_eq!(
        qname(m, uses[0].from),
        "src::orders.service::OrdersService::create"
    );
    let ev = Evidence::of(uses[0]).expect("an EVIDENCE cell");
    assert_eq!(
        (ev.emitter.as_str(), ev.rule.as_deref()),
        ("extractor:anchor", Some("const_fold"))
    );
    // Shaped like the literal-keyed handler the per-file pass minted: the
    // same cells in the same order (POSITION, IMPORTS, then ORIGIN).
    let kinds = |q: &str| -> Vec<_> {
        m.graphs
            .iter()
            .flat_map(|g| g.nodes.iter().map(move |n| (g, n)))
            .filter(|(g, n)| g.nav.qname_by_id.get(&n.id).is_some_and(|x| x == q))
            .flat_map(|(_, n)| n.cells.iter().map(|c| c.kind))
            .collect()
    };
    assert_eq!(
        kinds("event_emit:order.created"),
        kinds("event_handle:order.created")
    );
    assert_eq!(
        kinds("event_emit:order.created"),
        vec![cell_type::POSITION, cell_type::IMPORTS, cell_type::ORIGIN]
    );
    assert_eq!(
        event_const_marker(td.path()),
        vec![s(
            "[event-const] folded 1 event sites in 1 files (unresolved=0)"
        )]
    );
}

#[test]
fn same_file_constant_folds() {
    // A same-file binding resolves at any shape; one UPPER_SNAKE segment too.
    let td = repo(&[(
        "src/orders.service.ts",
        r#"import { EventEmitter2 } from "@nestjs/event-emitter";

const ORDER_PLACED = "order.placed";

export class OrdersService {
  constructor(private eventEmitter: EventEmitter2) {}

  place(x: string) {
    this.eventEmitter.emit(ORDER_PLACED, x);
  }
}
"#,
    )]);
    let r = build(td.path());
    assert_eq!(
        qnames_of(&r.merged, node_kind::EVENT_EMITTER),
        vec![s("event_emit:order.placed")]
    );
}

#[test]
fn ambiguous_constant_keeps_its_path() {
    // Two files bind `Events.Created` to different literals: the strict
    // lookup refuses the key, so the site keeps its constant path.
    let td = repo(&[
        (
            "src/a.events.ts",
            "export const Events = { Created: \"a.created\" } as const;\n",
        ),
        (
            "src/b.events.ts",
            "export const Events = { Created: \"b.created\" } as const;\n",
        ),
        (
            "src/c.service.ts",
            r#"import { EventEmitter2 } from "@nestjs/event-emitter";
import { Events } from "./a.events";

export class CService {
  constructor(private eventEmitter: EventEmitter2) {}

  fire(x: string) {
    this.eventEmitter.emit(Events.Created, x);
  }
}
"#,
        ),
    ]);
    let r = build(td.path());
    assert_eq!(
        qnames_of(&r.merged, node_kind::EVENT_EMITTER),
        vec![s("event_emit:Events.Created")]
    );
    assert_eq!(
        event_const_marker(td.path()),
        vec![s(
            "[event-const] folded 0 event sites in 0 files (unresolved=1)"
        )]
    );
}

#[test]
fn unbound_constant_keeps_its_path() {
    let td = repo(&[
        ("src/orders.events.ts", ORDERS_EVENTS),
        ("src/unbound.service.ts", UNBOUND_SERVICE),
    ]);
    let r = build(td.path());
    assert_eq!(
        qnames_of(&r.merged, node_kind::EVENT_EMITTER),
        vec![s("event_emit:Unbound.Thing")]
    );
    assert!(edges(&r.merged, edge_category::EVENT_FLOWS).is_empty());
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

#[test]
fn incremental_equals_clean() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("repo");
    write(&root, "src/orders.events.ts", ORDERS_EVENTS);
    write(&root, "src/orders.service.ts", ORDERS_SERVICE);
    write(&root, "src/audit.listener.ts", AUDIT_LISTENER);
    let path = root.to_str().unwrap();

    let mut cache = ParseCache::new();
    let cold = generate_one_with_cache(path, &mut cache).expect("cold");
    assert_eq!(
        qnames_of(&cold.merged, node_kind::EVENT_EMITTER),
        vec![s("event_emit:order.created")]
    );

    // Re-bind the constant: only its own file changes.
    write(
        &root,
        "src/orders.events.ts",
        "export const OrderEvents = { Created: \"order.placed\", Paid: \"order.paid\" } as const;\n",
    );
    let warm = generate_one_with_cache(path, &mut cache).expect("warm");
    let diff = cache.last_diff().expect("a diff").clone();
    assert!(
        diff.reused.contains(&s("src/orders.service.ts")),
        "{diff:?}"
    );
    assert_eq!(diff.reparsed, vec![s("src/orders.events.ts")]);
    // The cache keeps CB.3a's pre-fold parse of the reused file ...
    let cached = cache
        .iter()
        .find(|c| c.path == "src/orders.service.ts")
        .expect("cached parse");
    assert!(
        cached
            .parse
            .nav
            .qname_by_id
            .values()
            .any(|q| q == "event_emit:OrderEvents.Created"),
        "the parse cache stores the constant-path node"
    );
    // ... yet the graph is keyed by the NEW literal.
    assert_eq!(
        qnames_of(&warm.merged, node_kind::EVENT_EMITTER),
        vec![s("event_emit:order.placed")]
    );
    assert!(edges(&warm.merged, edge_category::EVENT_FLOWS).is_empty());

    let clean = generate_one(path).expect("clean");
    assert_eq!(
        gmap_bytes(&warm, &td.path().join("warm")),
        gmap_bytes(&clean, &td.path().join("clean")),
        "incremental == clean"
    );
}

#[test]
fn overlay_pin_names_an_env_constant() {
    let service = r#"import { EventEmitter2 } from "@nestjs/event-emitter";

export const ORDER_EVENT = process.env.ORDER_EVENT;

export class OrdersService {
  constructor(private eventEmitter: EventEmitter2) {}

  create(x: string) {
    this.eventEmitter.emit(ORDER_EVENT, x);
  }
}
"#;
    let td = repo(&[
        ("src/orders.service.ts", service),
        ("src/audit.listener.ts", AUDIT_LISTENER),
    ]);
    // Control: the scan refuses the env read, so nothing binds the constant.
    let r = build(td.path());
    assert_eq!(
        qnames_of(&r.merged, node_kind::EVENT_EMITTER),
        vec![s("event_emit:ORDER_EVENT")]
    );
    assert!(edges(&r.merged, edge_category::EVENT_FLOWS).is_empty());

    // The overlay pin binds it.
    write(
        td.path(),
        ".glia/overlay.toml",
        "version = 1\n\n[constants]\nORDER_EVENT = \"order.created\"\n",
    );
    let r = build(td.path());
    assert_eq!(
        qnames_of(&r.merged, node_kind::EVENT_EMITTER),
        vec![s("event_emit:order.created")]
    );
    assert_eq!(
        edges(&r.merged, edge_category::EVENT_FLOWS),
        vec![pair(
            "event_emit:order.created",
            "event_handle:order.created"
        )]
    );
}
