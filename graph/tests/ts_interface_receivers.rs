//! GD.2 (FD.9): TypeScript interface members as METHODs, end to end through
//! the TS family's graph build. The parser mints a `method_signature` and a
//! function-typed `property_signature` as METHOD children of the INTERFACE;
//! the build files them in `interface_methods` (A6.6), so a call through an
//! interface-typed parameter or a private `#field` of that type binds the
//! interface's own METHOD, and each `implements` class's same-named METHOD
//! pairs with it by method-level IMPLEMENTS.

use glia_core::{NodeId, RepoId};
use glia_graph::build_typescript_family;
use glia_parser_typescript::{FileParse, GRAPH_TYPE, edge_category, node_kind, parse_file};

fn repo() -> RepoId {
    RepoId::from_canonical("test://ts_interface_receivers")
}

fn parse(src: &str, rel: &str, qname: &str) -> FileParse {
    parse_file(src, rel, qname, repo()).unwrap()
}

fn id(kind: glia_core::NodeKindId, qname: &str) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
}

const STORE: &str = "\
export interface Store {
  load(id: string): string;
  save: (v: string) => void;
  size: number;
}
";

const PG: &str = "\
import { Store } from './store';
export class PgStore implements Store {
  load(id: string) { return id; }
  save(v: string) {}
  size = 0;
}
";

const MAIN: &str = "\
import { Store } from './store';
export function run(s: Store) { s.load('x'); s.save('y'); }
export class Holder {
  #store: Store;
  constructor(s: Store) { this.#store = s; }
  go() { this.#store.load('z'); }
}
";

#[test]
fn interface_members_bind_receivers_and_pair_implementations() {
    let files = vec![
        parse(STORE, "store.ts", "store"),
        parse(PG, "pg.ts", "pg"),
        parse(MAIN, "main.ts", "main"),
    ];
    let g = build_typescript_family(repo(), files, |_, raw| {
        raw.strip_prefix("./").map(str::to_string)
    })
    .unwrap();
    let iface = id(node_kind::INTERFACE, "store::Store");
    let load = id(node_kind::METHOD, "store::Store::load");
    let save = id(node_kind::METHOD, "store::Store::save");

    let mut members: Vec<(&str, NodeId)> = g
        .symbols
        .interface_methods
        .get(&iface)
        .expect("Store has interface methods")
        .iter()
        .map(|(n, m)| (n.as_str(), *m))
        .collect();
    members.sort_by_key(|(n, _)| *n);
    assert_eq!(members, vec![("load", load), ("save", save)]);
    assert!(
        !g.symbols.class_methods.contains_key(&iface),
        "an interface's members never enter class_methods"
    );

    let calls: Vec<(NodeId, NodeId)> = g
        .edges
        .iter()
        .filter(|e| e.category == edge_category::CALLS)
        .map(|e| (e.from, e.to))
        .collect();
    let run = id(node_kind::FUNCTION, "main::run");
    let go = id(node_kind::METHOD, "main::Holder::go");
    assert!(calls.contains(&(run, load)), "s.load(): {calls:?}");
    assert!(calls.contains(&(run, save)), "s.save(): {calls:?}");
    assert!(calls.contains(&(go, load)), "this.#store.load(): {calls:?}");

    let implements: Vec<(NodeId, NodeId)> = g
        .edges
        .iter()
        .filter(|e| e.category == edge_category::IMPLEMENTS)
        .map(|e| (e.from, e.to))
        .collect();
    let pg_load = id(node_kind::METHOD, "pg::PgStore::load");
    let pg_save = id(node_kind::METHOD, "pg::PgStore::save");
    assert!(implements.contains(&(pg_load, load)), "{implements:?}");
    assert!(implements.contains(&(pg_save, save)), "{implements:?}");
    assert!(
        implements.contains(&(id(node_kind::CLASS, "pg::PgStore"), iface)),
        "the class-level edge stays"
    );
}
