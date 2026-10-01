//! CG.4b (CA-13) end to end: an external HTTP endpoint stays out of pairing
//! and says so.
//!
//! One root holding a web client (`web/`, its own package.json) and an
//! Express server (`server/`) that serves `/search`, `/products` and
//! `/orders`, shaped like the `http-external-host` bench fixture. The client
//! calls `https://nominatim.openstreetmap.org/search` from the call's own
//! literal (CG.4a marks that site `"external":true`), `/products` through
//! `environment.apiUrl` and `/orders` at the same host written literally
//! (configured by `environment.apiUrl`, so never marked).
//!
//! The HTTP resolver leaves `/search` unpaired although the server serves
//! `GET /search`, `tag_synthetic_provenance` stamps it ORIGIN `external`,
//! `glia gaps` lists it neither as an unpaired endpoint nor as a suspected
//! edge, and the overlay `[constants]` pin (the escape hatch) makes it the
//! build's own call again: unmarked, unstamped, paired.
//!
//! CJ.3: the two answers that list sinks and servers read that verdict.
//! `effects` labels the `/search` sink with its host (`external_hosts`), and
//! `serves "GET /search"` lists the external ENDPOINT after the in-repo route,
//! match `external`.

use std::path::Path;

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, NodeId};
use glia_engine::effects::{EffectRow, Effects, EffectsArgs, effects};
use glia_engine::gaps::{GapsOptions, GapsReport, SUSPECTED_EDGE, UNPAIRED_ENDPOINT, gaps_report};
use glia_engine::serves::serves;
use glia_engine::{GenerateResult, generate_one};

const APP_JS: &str = "const express = require('express');

const app = express();

app.get('/search', (req, res) => res.json([]));
app.get('/products', (req, res) => res.json([]));
app.get('/orders', (req, res) => res.json([]));

app.listen(3000);
";
const SERVER_PACKAGE: &str =
    "{\"name\":\"server\",\"version\":\"1.0.0\",\"dependencies\":{\"express\":\"^4.18.0\"}}\n";
const WEB_PACKAGE: &str = "{\"name\":\"web\",\"version\":\"1.0.0\"}\n";
const ENVIRONMENT_TS: &str = "export const environment = {
  apiUrl: 'https://api.shop.io',
};
";
const GEO_TS: &str = "import { environment } from './environments/environment';

export async function forwardGeocode(q: string) {
  return fetch(`https://nominatim.openstreetmap.org/search?q=${q}&format=json`);
}

export async function listProducts() {
  return fetch(`${environment.apiUrl}/products`);
}

export async function listOrders() {
  return fetch('https://api.shop.io/orders');
}
";
/// The escape hatch: a pinned URL configures its site whatever the key.
const PIN: &str = "version = 1

[constants]
GEO_BASE = \"https://nominatim.openstreetmap.org\"
";

const SEARCH: &str = "endpoint:GET:/search @web";

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
}

fn fixture(pinned: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    let root = d.path();
    write(root, "server/app.js", APP_JS);
    write(root, "server/package.json", SERVER_PACKAGE);
    write(root, "web/package.json", WEB_PACKAGE);
    write(root, "web/src/environments/environment.ts", ENVIRONMENT_TS);
    write(root, "web/src/geo.ts", GEO_TS);
    if pinned {
        write(root, ".glia/overlay.toml", PIN);
    }
    d
}

fn build(root: &Path) -> GenerateResult {
    generate_one(&root.to_string_lossy()).expect("build")
}

/// The one node of `kind` named `qname`.
fn node(r: &GenerateResult, kind: glia_core::NodeKindId, qname: &str) -> NodeId {
    let mut hits: Vec<NodeId> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter(move |n| {
                g.nav.kind_by_id.get(&n.id) == Some(&kind)
                    && g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname)
            })
        })
        .map(|n| n.id)
        .collect();
    hits.dedup();
    assert_eq!(hits.len(), 1, "one {qname}: {:?}", all_qnames(r, kind));
    hits[0]
}

fn all_qnames(r: &GenerateResult, kind: glia_core::NodeKindId) -> Vec<String> {
    r.merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&kind))
                .filter_map(move |n| g.nav.qname_by_id.get(&n.id).cloned())
        })
        .collect()
}

/// The HTTP_CALLS targets of `from`, by qname.
fn http_targets(r: &GenerateResult, from: NodeId) -> Vec<String> {
    let mut out: Vec<String> = r
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::HTTP_CALLS && e.from == from)
        .filter_map(|e| {
            r.merged
                .graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&e.to).cloned())
        })
        .collect();
    out.sort();
    out
}

/// A cell payload of `cell` on any entry of `id`, as text.
fn payloads(r: &GenerateResult, id: NodeId, cell: glia_core::CellTypeId) -> Vec<String> {
    r.merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter(|n| n.id == id)
        .flat_map(|n| &n.cells)
        .filter(|c| c.kind == cell)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) | CellPayload::Text(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

fn report(r: &GenerateResult, category: Option<&str>) -> GapsReport {
    let roots: Vec<(u64, std::path::PathBuf)> = r
        .repo_roots
        .iter()
        .map(|(id, p)| (*id, std::path::PathBuf::from(p)))
        .collect();
    let mut opts = GapsOptions::default();
    opts.category = category.map(str::to_string);
    gaps_report(&r.merged, &roots, &opts).expect("known categories")
}

#[test]
fn external_endpoint_is_unpaired_labelled_and_no_gap() {
    let d = fixture(false);
    let r = build(d.path());

    let search = node(&r, node_kind::ENDPOINT, SEARCH);
    let hits = payloads(&r, search, cell_type::ENDPOINT_HIT);
    assert!(
        !hits.is_empty() && hits.iter().all(|h| h.contains("\"external\":true")),
        "CG.4a marks every site: {hits:?}"
    );
    assert!(
        http_targets(&r, search).is_empty(),
        "nominatim's /search is not GET /search"
    );
    assert_eq!(
        payloads(&r, search, cell_type::ORIGIN),
        vec![r#"{"provenance":"external"}"#.to_string()],
        "provenance external"
    );
    // It keeps its caller.
    assert!(
        r.merged
            .all_edges()
            .any(|e| e.category == edge_category::CALLS && e.to == search),
        "forwardGeocode still CALLS it"
    );

    // Controls: a host from a configured constant, and the same host
    // written literally, pair as before, and carry no ORIGIN.
    for (ep, route) in [
        ("endpoint:GET:/products @web", "GET /products @server"),
        ("endpoint:GET:/orders @web", "GET /orders @server"),
    ] {
        let id = node(&r, node_kind::ENDPOINT, ep);
        assert_eq!(http_targets(&r, id), vec![route.to_string()], "{ep}");
        assert!(payloads(&r, id, cell_type::ORIGIN).is_empty(), "{ep}");
    }

    // gaps: not an unpaired endpoint, and never a suspected edge, though
    // /products and /orders teach CD.3b the ENDPOINT-HTTP_CALLS->ROUTE
    // triple and GET /search @server is unpaired.
    let rep = report(&r, None);
    let unpaired: Vec<&str> = rep
        .rows
        .iter()
        .filter(|row| row.category == UNPAIRED_ENDPOINT)
        .map(|row| row.qname.as_str())
        .collect();
    assert!(!unpaired.contains(&SEARCH), "{rep:#?}");
    let suspected = report(&r, Some(SUSPECTED_EDGE));
    assert!(
        suspected
            .rows
            .iter()
            .all(|row| !row.qname.starts_with("endpoint:GET:/search")),
        "{suspected:#?}"
    );
}

/// The overlay `[constants]` pin configures nominatim's site: the call is the
/// build's own again - unmarked, unstamped, paired with GET /search.
#[test]
fn overlay_pin_brings_the_endpoint_back() {
    let d = fixture(true);
    let r = build(d.path());

    let search = node(&r, node_kind::ENDPOINT, SEARCH);
    let hits = payloads(&r, search, cell_type::ENDPOINT_HIT);
    assert!(hits.iter().all(|h| !h.contains("\"external\"")), "{hits:?}");
    assert!(payloads(&r, search, cell_type::ORIGIN).is_empty());
    assert_eq!(
        http_targets(&r, search),
        vec!["GET /search @server".to_string()]
    );
}

const NOMINATIM: &str = "nominatim.openstreetmap.org";

fn effects_of(r: &GenerateResult, seed: &str) -> Effects {
    effects(&r.merged, &r.repo_labels, &[seed], &EffectsArgs::default()).expect("effects")
}

/// The one row of `a` whose sink is `qname`.
fn sink<'a>(a: &'a Effects, qname: &str) -> &'a EffectRow {
    let rows: Vec<&EffectRow> = a.effects.iter().filter(|r| r.qname == qname).collect();
    assert_eq!(rows.len(), 1, "one {qname}: {a:#?}");
    rows[0]
}

/// One server row: `(qname, kind, match, external_hosts)`.
type Row = (String, &'static str, &'static str, Vec<String>);

/// The servers of `channel`. An external row binds no handler (it sits
/// outside the build), and an answer with rows carries no absence.
fn served(r: &GenerateResult, channel: &str) -> Vec<Row> {
    let a = serves(&r.merged, channel, "auto").expect("serves");
    if a.results.is_empty() {
        assert!(
            a.absence.is_some(),
            "{channel}: an empty answer carries its absence"
        );
    } else {
        assert!(
            a.absence.is_none(),
            "{channel}: served, so no absence: {a:#?}"
        );
    }
    for s in a.results.iter().filter(|s| s.r#match == "external") {
        assert!(s.handlers.is_empty(), "{channel}: {s:#?}");
        assert!(
            s.line.is_some_and(|l| l >= 1),
            "{channel}: located, 1-based: {s:#?}"
        );
    }
    a.results
        .iter()
        .map(|s| (s.qname.clone(), s.kind, s.r#match, s.external_hosts.clone()))
        .collect()
}

/// CJ.3: effects reads CG.4b's ORIGIN verdict. The external `/search` sink
/// names its host and has no receiver; the paired `/orders` sink names no
/// host and its route; with the `[constants]` pin `/search` is in-repo again.
#[test]
fn effects_labels_the_external_sink() {
    let d = fixture(false);
    let r = build(d.path());

    let geo = effects_of(&r, "web::src::geo::forwardGeocode");
    let search = sink(&geo, SEARCH);
    assert_eq!(search.class, "http_call");
    assert_eq!(search.external_hosts, vec![NOMINATIM.to_string()]);
    assert!(
        search.downstream.is_empty(),
        "nothing pairs it: {search:#?}"
    );

    let orders = effects_of(&r, "web::src::geo::listOrders");
    let row = sink(&orders, "endpoint:GET:/orders @web");
    assert!(row.external_hosts.is_empty(), "{row:#?}");
    let receivers: Vec<&str> = row.downstream.iter().map(|t| t.qname.as_str()).collect();
    assert_eq!(receivers, ["GET /orders @server"]);

    let pinned = fixture(true);
    let r = build(pinned.path());
    let geo = effects_of(&r, "web::src::geo::forwardGeocode");
    let search = sink(&geo, SEARCH);
    assert!(
        search.external_hosts.is_empty(),
        "the pin configures the site: {search:#?}"
    );
    let receivers: Vec<&str> = search.downstream.iter().map(|t| t.qname.as_str()).collect();
    assert_eq!(receivers, ["GET /search @server"]);
}

/// CJ.3: serves lists the external ENDPOINT after the in-repo route, match
/// `external`, with its host and no handler; a URL naming another host, a
/// path no external call names, and the pinned build list the route alone.
#[test]
fn serves_lists_the_external_host_after_the_route() {
    let d = fixture(false);
    let r = build(d.path());

    let route = |q: &str| -> Row { (q.to_string(), "ROUTE", "exact", Vec::new()) };
    let external: Row = (
        SEARCH.to_string(),
        "ENDPOINT",
        "external",
        vec![NOMINATIM.to_string()],
    );
    let both = vec![route("GET /search @server"), external];
    assert_eq!(served(&r, "GET /search"), both);
    assert_eq!(
        served(&r, "GET https://nominatim.openstreetmap.org/search"),
        both
    );
    // The asked host is read as the fold reads one: case, port and query folded.
    assert_eq!(
        served(&r, "GET https://Nominatim.OpenStreetMap.org:443/search?q=x"),
        both
    );
    assert_eq!(
        served(&r, "GET https://other.example.org/search"),
        vec![route("GET /search @server")]
    );
    assert_eq!(
        served(&r, "GET /orders"),
        vec![route("GET /orders @server")]
    );
    // A bare path asks every verb: the same two rows.
    assert_eq!(served(&r, "/search"), both);

    let pinned = fixture(true);
    let r = build(pinned.path());
    assert_eq!(
        served(&r, "GET /search"),
        vec![route("GET /search @server")]
    );
}
