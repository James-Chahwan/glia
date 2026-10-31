//! GD.2 (FD.9) — TypeScript interface members in `glia gaps` dead_symbol and
//! liveness. An interface's `method_signature` / function-typed property is a
//! METHOD child of the INTERFACE; a call through an interface-typed receiver
//! binds it, and each `implements` class's same-named METHOD IMPLEMENTS it.
//!
//! So, with `main -> run -> s.load()` live: `Store::load` is live, and
//! `PgStore::load` is no row (CH.1c dispatch: its interface member has an
//! incoming CALLS from another node). An interface member is never a
//! dead_symbol row itself (it is reported with its interface, which is no
//! dead_symbol kind): `Store::unused`, which nothing calls, is no row, while
//! its implementation `PgStore::unused`, which nothing reaches, still is.

use std::path::Path;

use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, gaps_report};
use glia_engine::{entrypoint_reachable, generate_one};
use glia_graph::MergedGraph;

const STORE_TS: &str = "\
export interface Store {
  load(id: string): string;
  save: (v: string) => void;
  unused(): void;
}
";

const PG_TS: &str = "\
import { Store } from './store';

export class PgStore implements Store {
  load(id: string) {
    return id;
  }
  save(v: string) {}
  unused() {}
}
";

const MAIN_TS: &str = "\
import { Store } from './store';
import { PgStore } from './pg';

export function run(s: Store) {
  s.load('x');
  s.save('y');
}

export function idle() {}

export function main() {
  run(new PgStore());
}
";

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

fn build() -> MergedGraph {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, body) in [
        ("package.json", "{\"name\": \"store\"}\n"),
        ("src/store.ts", STORE_TS),
        ("src/pg.ts", PG_TS),
        ("src/main.ts", MAIN_TS),
    ] {
        write(dir.path(), rel, body);
    }
    let root = dir.path().to_str().expect("utf-8 temp path").to_string();
    generate_one(&root)
        .unwrap_or_else(|e| panic!("generate_one: {e}"))
        .merged
}

/// The id of the node whose qname ends with `suffix` (the first one).
fn node(m: &MergedGraph, suffix: &str) -> glia_core::NodeId {
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(move |n| (g, n)))
        .find(|(g, n)| {
            g.nav
                .qname_by_id
                .get(&n.id)
                .is_some_and(|q| q.ends_with(suffix))
        })
        .map(|(_, n)| n.id)
        .unwrap_or_else(|| panic!("a node ending `{suffix}`"))
}

#[test]
fn interface_members_are_live_through_calls_and_never_dead_rows() {
    let m = build();
    let live = entrypoint_reachable(&m);
    for reached in [
        "::main::main",
        "::main::run",
        "::Store::load",
        "::Store::save",
    ] {
        assert!(live.contains(&node(&m, reached)), "{reached} is live");
    }
    let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
    let rows: Vec<&str> = rep
        .rows
        .iter()
        .filter(|r| r.category == DEAD_SYMBOL)
        .map(|r| r.qname.as_str())
        .collect();
    let has = |suffix: &str| rows.iter().any(|q| q.ends_with(suffix));
    for gone in [
        "::Store::load",
        "::Store::save",
        "::Store::unused",
        "::PgStore::load",
        "::PgStore::save",
    ] {
        assert!(!has(gone), "{gone} is no dead_symbol row: {rows:?}");
    }
    for control in ["::PgStore::unused", "::idle"] {
        assert!(has(control), "nothing reaches {control}: a row, {rows:?}");
    }
}
