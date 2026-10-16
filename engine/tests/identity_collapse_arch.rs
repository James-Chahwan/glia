//! DD.14: `glia arch` reads identity as classes. A SAME_AS class (every tier)
//! is one link per pair of the services its members sit in, at the class's
//! weakest edge; every other link counts the distinct endpoint pairs once
//! their ends are mapped through the FACT / DERIVED classes (DD.8's
//! `IdentityView` collapse set), so one contract op declared twice documents
//! its route once. `Collapse::off()` is the raw member -> center star, and a
//! graph with no SAME_AS edge answers exactly as before.
//!
//! Real `generate_many` builds: tempdir repos, and DD.6's
//! `identity-vendored-subtree` fixture.
//!
//! Baseline before DD.14 (HEAD bda10135): this file did not compile
//! (`arch::service_map_collapsed` did not exist). Measured with that HEAD's
//! `glia arch --include-shared --json`: the three react repos listed 0 links
//! (`unplaced=2`: a PACKAGE_DEP has no file); the vendored pair 4 SAME_AS links
//! `shop -> money` (star direction, SAME_AS not symmetric); the two OpenAPI
//! copies `web -> api DOCUMENTS count 2`.

use std::path::{Path, PathBuf};

use glia_activation::algo::classes::Classes;
use glia_code_domain::edge_category;
use glia_engine::arch::{ServiceLink, ServiceMap, default_keying, service_map_collapsed};
use glia_engine::identity::Collapse;
use glia_engine::{GenerateResult, generate_many, generate_one, service_map, service_map_with};

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, text).expect("write");
}

fn build(paths: &[PathBuf]) -> GenerateResult {
    let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    generate_many(&paths).unwrap_or_else(|e| panic!("{paths:?} builds: {e}"))
}

fn arch(r: &GenerateResult, collapse: &Collapse) -> ServiceMap {
    service_map_collapsed(&r.merged, &r.repo_labels, &default_keying(&r.merged), collapse)
}

fn on(r: &GenerateResult) -> Collapse {
    Collapse::with_labels(&r.repo_labels)
}

/// `(from, to, channel, count, confidence)` of every link over `mechanism`.
fn rows<'a>(map: &'a ServiceMap, mechanism: &str) -> Vec<(&'a str, &'a str, &'a str, usize, &'a str)> {
    map.links
        .iter()
        .filter(|l| l.mechanism == mechanism)
        .map(|l: &ServiceLink| (l.from.as_str(), l.to.as_str(), l.channel.as_str(), l.count, l.confidence))
        .collect()
}

fn json(map: &ServiceMap) -> String {
    serde_json::to_string(map).expect("the map serialises")
}

/// The classes the build's SAME_AS edges form, any tier.
fn same_as_classes(r: &GenerateResult) -> Classes {
    Classes::from_pairs(
        r.merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SAME_AS)
            .map(|e| (e.from, e.to)),
    )
}

/// Three repos declaring react, each with its own code: one package_name class
/// (FACT) of three PACKAGE_DEPs, stored as a two-edge star.
fn three_react_repos() -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().expect("tempdir");
    let mut paths = Vec::new();
    for repo in ["alpha", "beta", "gamma"] {
        let root = td.path().join(repo);
        write(
            &root,
            "package.json",
            &format!("{{\"name\": \"{repo}\", \"version\": \"1.0.0\", \"dependencies\": {{\"react\": \"^18.2.0\"}}}}\n"),
        );
        write(&root, "src/index.js", &format!("export function {repo}Main() {{\n  return \"{repo}\";\n}}\n"));
        paths.push(root);
    }
    let r = build(&paths);
    (td, r)
}

#[test]
fn three_repo_class_links_every_pair() {
    let (_td, r) = three_react_repos();
    let classes = same_as_classes(&r);
    assert_eq!(classes.len(), 1, "control: one react class");
    assert_eq!(classes.members(), 3);
    let star = r.merged.cross_edges.iter().filter(|e| e.category == edge_category::SAME_AS).count();
    assert_eq!(star, 2, "control: stored as a two-edge star");

    let map = arch(&r, &on(&r));
    assert_eq!(map.keying, "per_repo");
    assert_eq!(
        rows(&map, "SAME_AS"),
        [
            ("alpha", "beta", "react", 1, "strong"),
            ("alpha", "gamma", "react", 1, "strong"),
            ("beta", "gamma", "react", 1, "strong"),
        ],
        "one row per service pair of the class: {:#?}",
        map.links
    );
    for l in &map.links {
        assert_eq!(
            (l.example_from_qname.as_str(), l.example_to_qname.as_str()),
            ("package:npm:react", "package:npm:react")
        );
    }
    // The PACKAGE_DEPs place the links, never the summaries.
    let off = arch(&r, &Collapse::off());
    let summaries = |m: &ServiceMap| {
        m.services.iter().map(|s| (s.id.clone(), s.nodes, s.files)).collect::<Vec<_>>()
    };
    assert_eq!(summaries(&map), summaries(&off));
    assert_eq!(map.unlocated_nodes, off.unlocated_nodes, "still unlocated");

    // Off: the raw star, two links, both touching the center's service.
    let raw = rows(&off, "SAME_AS");
    assert_eq!(raw.len(), 2, "{raw:#?}");
    assert!(raw.iter().all(|&(_, _, c, n, conf)| (c, n, conf) == ("react", 1, "strong")));
    let center = ["alpha", "beta", "gamma"]
        .into_iter()
        .find(|s| raw.iter().all(|&(f, t, ..)| f == *s || t == *s))
        .expect("both star links share the center's service");
    assert!(raw.iter().all(|&(f, t, ..)| f < t), "SAME_AS is symmetric: from < to ({raw:?}, center {center})");

    // service_map / service_map_with collapse by default (no labels needed).
    assert!(Collapse::default().enabled);
    let dflt = arch(&r, &Collapse::default());
    assert_eq!(json(&service_map(&r.merged, &r.repo_labels)), json(&dflt));
    assert_eq!(json(&service_map_with(&r.merged, &r.repo_labels, &default_keying(&r.merged))), json(&dflt));
    assert_eq!(json(&dflt), json(&map), "the labels only name also_in rows, which arch has none of");
}

#[test]
fn heuristic_links_weak() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures/identity-vendored-subtree");
    let r = build(&[fixture.join("money"), fixture.join("shop")]);
    let classes = same_as_classes(&r);
    assert!(classes.len() >= 2, "control: the vendored candidates, {} classes", classes.len());

    let map = arch(&r, &on(&r));
    let links = rows(&map, "SAME_AS");
    assert_eq!(links.len(), classes.len(), "one row per (service pair, class): {links:#?}");
    assert!(
        links.iter().all(|&(f, t, _, n, conf)| (f, t, n, conf) == ("money", "shop", 1, "weak")),
        "every candidate class money <-> shop, from < to, HEURISTIC = weak: {links:#?}"
    );
    let mut channels: Vec<&str> = links.iter().map(|l| l.2).collect();
    channels.sort_unstable();
    channels.dedup();
    assert_eq!(channels.len(), links.len(), "one class per channel: {channels:?}");
    assert!(channels.contains(&"formatCents") && channels.contains(&"parseCents"), "{channels:?}");
    let total: usize = links.iter().map(|l| l.3).sum();
    assert_eq!(total, classes.len(), "the link counts sum to the vendored classes");

    // Candidates never collapse, so every two-member class reads as its one
    // star edge did: Off is the same rows, now oriented from < to as well.
    let off = arch(&r, &Collapse::off());
    assert_eq!(rows(&off, "SAME_AS"), links);
}

/// `api` serves `GET /orders` (Flask); `web` holds two OpenAPI copies of the
/// op in two directories under one package.json project: one contract_op +
/// contract_route class (DERIVED), each copy DOCUMENTS the route.
#[test]
fn flow_counts_by_class() {
    let td = tempfile::tempdir().expect("tempdir");
    let api = td.path().join("api");
    write(
        &api,
        "app.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
         @app.route(\"/orders\", methods=[\"GET\"])\ndef list_orders():\n    return {\"orders\": []}\n",
    );
    let web = td.path().join("web");
    write(&web, "package.json", "{\"name\": \"web\", \"version\": \"1.0.0\"}\n");
    write(
        &web,
        "docs/a/openapi.yaml",
        "openapi: 3.0.0\ninfo:\n  title: Orders\n  version: 1.0.0\npaths:\n  /orders:\n    get:\n      \
         operationId: listOrders\n      responses:\n        '200':\n          description: ok\n",
    );
    write(
        &web,
        "docs/b/openapi.json",
        "{\"openapi\": \"3.0.0\", \"info\": {\"title\": \"Orders\", \"version\": \"1.0.0\"}, \"paths\": \
         {\"/orders\": {\"get\": {\"operationId\": \"listOrders\", \"responses\": {\"200\": \
         {\"description\": \"ok\"}}}}}}\n",
    );
    let r = build(&[api, web]);
    let classes = same_as_classes(&r);
    assert_eq!((classes.len(), classes.members()), (1, 2), "control: the two ops are one class");

    let map = arch(&r, &on(&r));
    assert_eq!(
        rows(&map, "DOCUMENTS"),
        [("web", "api", "GET /orders", 1, "medium")],
        "one op, documenting its route once: {:#?}",
        map.links
    );
    assert!(rows(&map, "SAME_AS").is_empty(), "the class sits in web: a self link");
    let off = arch(&r, &Collapse::off());
    assert_eq!(rows(&off, "DOCUMENTS"), [("web", "api", "GET /orders", 2, "medium")], "control: one per copy");
    assert_eq!(map.self_links, off.self_links, "the SAME_AS self link either way");
    assert_eq!(map.self_links, 1);
}

/// A graph with no SAME_AS edge: on, off and the HEAD entry points are one
/// answer, byte for byte (`arch_service_map.rs` / `service_map.rs` pin it).
#[test]
fn class_free_identical() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../bench/substrate-gap/fixtures/xstack-ts-go-http");
    let r = generate_one(fixture).expect("fixture builds");
    assert!(
        !r.merged.cross_edges.iter().any(|e| e.category == edge_category::SAME_AS),
        "control: class-free"
    );
    let head = json(&service_map(&r.merged, &r.repo_labels));
    assert!(head.contains("\"HTTP_CALLS\""), "{head}");
    assert_eq!(json(&arch(&r, &on(&r))), head);
    assert_eq!(json(&arch(&r, &Collapse::off())), head);
}
