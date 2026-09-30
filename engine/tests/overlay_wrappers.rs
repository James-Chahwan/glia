//! LF.2e: `.glia/overlay.toml` `[[wrapper]]` stanzas mint the sink at every
//! call site of a hand-rolled HTTP / queue wrapper (or a client receiver whose
//! verb members are the calls), anchored to the enclosing function.
//!
//! Probe r1 (a `request(method, path)` wrapper over `fetch(path)`) and probe
//! w1 (`const api = makeClient(); api.get('/orders')`) against a flask API.
//! Pre-fix baseline (HEAD cb356fc): the wrapper yields only the inner
//! `endpoint:GET:<unresolved>`, `api.get('/orders')` mints nothing, and
//! `[http] routes=3 endpoints=0 paired=0`.

use std::path::{Path, PathBuf};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, EdgeCategoryId, Node, NodeId, NodeKindId};
use glia_engine::gaps::{
    GapsOptions, UNRESOLVED_ENDPOINT, WRAPPED_SINK, gaps_report, overlay_delta,
};
use glia_engine::{
    BuildOptions, GenerateResult, ParseCache, generate_many, generate_many_opts, generate_one,
    generate_one_with_cache,
};
use glia_store::write_merged_sharded;

const API_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef list_users():\n    return []\n\n\n@app.route(\"/users\", methods=[\"POST\"])\ndef create_user():\n    return {}\n\n\n@app.route(\"/orders\", methods=[\"GET\"])\ndef list_orders():\n    return []\n\n\n@app.route(\"/orders/<int:oid>\", methods=[\"DELETE\"])\ndef delete_order(oid):\n    return {}\n";

/// Probe r1 plus a POST call site.
const CLIENT_TS: &str = "export function request(method: string, path: string) {\n  return fetch(path, { method });\n}\n\nexport async function loadUsers() {\n  return request('GET', '/users');\n}\n\nexport async function createUser(u: unknown) {\n  return request(\"POST\", \"/users\");\n}\n";

const REQUEST_STANZA: &str = "version = 1\n\n[[wrapper]]\ncall = \"request\"\nkind = \"http\"\nmethod_arg = 0\npath_arg = 1\n";

/// A fresh, empty temp dir for one test.
fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("glia_lf2e_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

fn s(p: &Path) -> String {
    p.to_str().expect("utf-8 temp path").to_string()
}

/// `web/src/client.ts = client`, `web/.glia/overlay.toml = overlay` (when
/// given) and the flask `api/app.py`, built as two repos.
fn build_pair(
    tag: &str,
    client: &str,
    overlay: Option<&str>,
    opts: &BuildOptions,
) -> GenerateResult {
    let root = tmp(tag);
    write(&root, "web/src/client.ts", client);
    write(&root, "api/app.py", API_PY);
    if let Some(o) = overlay {
        write(&root, "web/.glia/overlay.toml", o);
    }
    let built = generate_many_opts(&[s(&root.join("web")), s(&root.join("api"))], false, opts);
    std::fs::remove_dir_all(&root).ok();
    built.expect("build")
}

fn pair(tag: &str, client: &str, overlay: &str) -> GenerateResult {
    build_pair(tag, client, Some(overlay), &BuildOptions::default())
}

fn qname_of(r: &GenerateResult, id: NodeId) -> String {
    r.merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// The first node instance named `qname`, with its kind.
fn node(r: &GenerateResult, qname: &str) -> Option<(Node, NodeKindId)> {
    r.merged.graphs.iter().find_map(|g| {
        g.nodes.iter().find_map(|n| {
            (g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname))
                .then(|| (n.clone(), *g.nav.kind_by_id.get(&n.id).expect("kind")))
        })
    })
}

/// Every ENDPOINT qname, sorted.
fn endpoints(r: &GenerateResult) -> Vec<String> {
    let mut out: Vec<String> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter_map(move |n| {
                (g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
                    .then(|| g.nav.qname_by_id.get(&n.id).cloned())
                    .flatten()
            })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `(from qname, to qname)` of every edge of `category`, sorted.
fn edges(r: &GenerateResult, category: EdgeCategoryId) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = r
        .merged
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (qname_of(r, e.from), qname_of(r, e.to)))
        .collect();
    out.sort();
    out
}

fn has_edge(r: &GenerateResult, from: &str, to: &str, category: EdgeCategoryId) -> bool {
    edges(r, category).iter().any(|(f, t)| f == from && t == to)
}

fn json_cells(cells: &[Cell], kind: glia_core::CellTypeId) -> Vec<serde_json::Value> {
    cells
        .iter()
        .filter(|c| c.kind == kind)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) => serde_json::from_str(j).ok(),
            _ => None,
        })
        .collect()
}

#[test]
fn http_wrapper_mints_endpoints() {
    let r = pair("http", CLIENT_TS, REQUEST_STANZA);
    let eps = endpoints(&r);
    for q in ["endpoint:GET:/users", "endpoint:POST:/users"] {
        assert!(eps.contains(&q.to_string()), "{q} minted: {eps:?}");
        let (n, kind) = node(&r, q).expect("node");
        assert_eq!(kind, node_kind::ENDPOINT);
        assert_eq!(n.confidence, Confidence::Weak, "an llm stanza mints Weak");
        let origin = json_cells(&n.cells, cell_type::ORIGIN);
        assert_eq!(
            origin,
            [serde_json::json!({"provenance": "overlay:llm", "rule": "wrapper#1"})],
            "{q}"
        );
        let hit = json_cells(&n.cells, cell_type::ENDPOINT_HIT);
        assert_eq!(hit.len(), 1, "{q}");
        assert_eq!(hit[0]["file"], "src/client.ts");
        assert_eq!(hit[0]["confidence"], "weak");
    }
    let hit_line = |q: &str| {
        json_cells(&node(&r, q).expect("node").0.cells, cell_type::ENDPOINT_HIT)[0]["line"].clone()
    };
    assert_eq!(hit_line("endpoint:GET:/users"), 6, "1-based call line");
    assert_eq!(hit_line("endpoint:POST:/users"), 10);

    // The CALLS edge from the enclosing function, with the stanza's evidence.
    assert!(has_edge(
        &r,
        "src::client::loadUsers",
        "endpoint:GET:/users",
        edge_category::CALLS
    ));
    assert!(has_edge(
        &r,
        "src::client::createUser",
        "endpoint:POST:/users",
        edge_category::CALLS
    ));
    let calls = r
        .merged
        .all_edges()
        .find(|e| e.category == edge_category::CALLS && qname_of(&r, e.to) == "endpoint:GET:/users")
        .expect("CALLS");
    let ev = Evidence::of(calls).expect("EVIDENCE");
    assert_eq!(
        (
            ev.emitter.as_str(),
            ev.rule.as_deref(),
            ev.file.as_deref(),
            ev.line
        ),
        (
            "overlay:wrapper",
            Some("wrapper#1"),
            Some("src/client.ts"),
            Some(5)
        )
    );

    // Paired with the flask routes of the other repo.
    let http = edges(&r, edge_category::HTTP_CALLS);
    assert!(
        http.contains(&("endpoint:GET:/users".into(), "GET /users".into())),
        "{http:?}"
    );
    assert!(
        http.contains(&("endpoint:POST:/users".into(), "POST /users".into())),
        "{http:?}"
    );
    let web = r
        .merged
        .graphs
        .iter()
        .find(|g| g.nav.qname_by_id.values().any(|q| q == "src::client"))
        .expect("web")
        .repo;
    let api = r
        .merged
        .graphs
        .iter()
        .find(|g| g.nav.qname_by_id.values().any(|q| q == "app::list_users"))
        .expect("api")
        .repo;
    assert_ne!(web, api, "two repos");
    // The wrapper's own inner fetch stays what the extractor made of it.
    assert!(
        eps.contains(&"endpoint:GET:<unresolved>".to_string()),
        "{eps:?}"
    );
}

#[test]
fn commented_call_is_skipped() {
    let client = format!(
        "{CLIENT_TS}\n// request('GET', '/legacy');\n/* request('GET', '/block'); */\n/**\n * request('GET', '/jsdoc')\n */\nexport function f() {{ g(); // request('GET', '/inline')\n}}\n"
    );
    let r = pair("comment", &client, REQUEST_STANZA);
    let eps = endpoints(&r);
    for q in ["/legacy", "/block", "/jsdoc", "/inline"] {
        assert!(
            !eps.contains(&format!("endpoint:GET:{q}")),
            "{q} is commented out: {eps:?}"
        );
    }
    assert!(eps.contains(&"endpoint:GET:/users".to_string()), "{eps:?}");
}

#[test]
fn definition_is_not_a_call_site() {
    // Literal defaults in the definitions must not read as call arguments.
    let client = "export function request(method = 'GET', path = '/def') {\n  return fetch(path, { method });\n}\n\nexport class Api {\n  request(method: string = 'PUT', path: string = '/typed'): Promise<Response> {\n    return fetch(path, { method });\n  }\n}\n\nexport async function loadUsers() {\n  return request('GET', '/users');\n}\n";
    let r = pair("def", client, REQUEST_STANZA);
    let eps = endpoints(&r);
    assert!(!eps.contains(&"endpoint:GET:/def".to_string()), "{eps:?}");
    assert!(!eps.contains(&"endpoint:PUT:/typed".to_string()), "{eps:?}");
    assert!(eps.contains(&"endpoint:GET:/users".to_string()), "{eps:?}");
}

#[test]
fn nonliteral_args_are_skipped() {
    let client = "export function request(method: string, path: string) {\n  return fetch(path, { method });\n}\n\nconst BASE = process.env.BASE;\n\nexport async function dyn(v: string, p: string) {\n  return request(v, p);\n}\n\nexport async function concat(id: string) {\n  return request('GET', '/users/' + id);\n}\n\nexport async function verbVar(m: string) {\n  return request(m, '/orders');\n}\n\nexport async function notAVerb() {\n  return request('FETCH', '/orders');\n}\n";
    let r = pair("nonlit", client, REQUEST_STANZA);
    assert_eq!(
        endpoints(&r),
        ["endpoint:GET:<unresolved>"],
        "no identity, no node"
    );
}

#[test]
fn receiver_form_mints_verb_endpoints() {
    let client = "const api = makeClient();\nconst apiClient = makeClient();\n\nexport async function listOrders() {\n  return api.get('/orders');\n}\n\nexport async function dropOrder() {\n  return api.delete('/orders/1');\n}\n\nexport async function dropById(id: string) {\n  return api.Delete(`/orders/${id}`);\n}\n\nexport async function typed() {\n  return api.get<Order[]>(`/orders`);\n}\n\nexport async function other() {\n  return apiClient.get('/x');\n}\n";
    let overlay = "version = 1\n\n[[wrapper]]\ncall = \"api\"\nkind = \"http\"\nreceiver = true\n";
    let r = pair("recv", client, overlay);
    assert_eq!(
        endpoints(&r),
        [
            "endpoint:DELETE:/orders/${…}",
            "endpoint:DELETE:/orders/1",
            "endpoint:GET:/orders"
        ],
        "apiClient is not api"
    );
    assert!(has_edge(
        &r,
        "endpoint:GET:/orders",
        "GET /orders",
        edge_category::HTTP_CALLS
    ));
    // A literal id is a value, not a placeholder: it pairs with no param route.
    let http = edges(&r, edge_category::HTTP_CALLS);
    assert!(
        !http.iter().any(|(f, _)| f == "endpoint:DELETE:/orders/1"),
        "{http:?}"
    );
    // The case-insensitive verb (`Delete(`) and a `${id}` path pair with the flask param route.
    assert!(
        http.iter()
            .any(|(f, t)| f == "endpoint:DELETE:/orders/${…}" && t.starts_with("DELETE /orders/")),
        "{http:?}"
    );
    // Two call sites of one identity: one node, a CALLS edge from each
    // (`api.get<Order[]>(...)`: the type arguments are stepped over).
    assert!(has_edge(
        &r,
        "src::client::listOrders",
        "endpoint:GET:/orders",
        edge_category::CALLS
    ));
    assert!(has_edge(
        &r,
        "src::client::typed",
        "endpoint:GET:/orders",
        edge_category::CALLS
    ));
    assert!(has_edge(
        &r,
        "src::client::dropOrder",
        "endpoint:DELETE:/orders/1",
        edge_category::CALLS
    ));
}

/// An extractor that already caught the call keeps its node: the same
/// identity at the same line is a duplicate, not a second node entry.
#[test]
fn extracted_call_is_a_duplicate() {
    // The TypeScript parser mints `this.x.get(...)` itself.
    let client = "export class Svc {\n  load() {\n    return this.api.get('/orders');\n  }\n}\n";
    let overlay = "version = 1\n\n[[wrapper]]\ncall = \"api\"\nkind = \"http\"\nreceiver = true\n";
    let with = pair("dup", client, overlay);
    let without = build_pair("dup-off", client, None, &BuildOptions::default());
    assert_eq!(endpoints(&with), ["endpoint:GET:/orders"]);
    let (n, _) = node(&with, "endpoint:GET:/orders").expect("node");
    assert!(
        json_cells(&n.cells, cell_type::ORIGIN).is_empty(),
        "the extracted node, untouched"
    );
    assert_eq!(
        edges(&with, edge_category::CALLS),
        edges(&without, edge_category::CALLS),
        "no second CALLS edge"
    );
}

#[test]
fn template_path_folds_through_the_const_table() {
    // The http half runs BEFORE the endpoint fold: a `${X}` wrapper path is
    // folded like a direct client call's.
    let client = "export const API = 'http://users-svc:8080';\n\nexport function request(method: string, path: string) {\n  return fetch(path, { method });\n}\n\nexport async function loadUsers() {\n  return request('GET', `${API}/users`);\n}\n";
    let r = pair("fold", client, REQUEST_STANZA);
    let eps = endpoints(&r);
    assert!(eps.contains(&"endpoint:GET:/users".to_string()), "{eps:?}");
    let hit = &json_cells(
        &node(&r, "endpoint:GET:/users").expect("node").0.cells,
        cell_type::ENDPOINT_HIT,
    )[0];
    assert_eq!(hit["template"], "${API}/users");
    assert_eq!(hit["host"], "users-svc:8080");
    assert!(has_edge(
        &r,
        "endpoint:GET:/users",
        "GET /users",
        edge_category::HTTP_CALLS
    ));
}

#[test]
fn queue_wrapper_mints_producer() {
    let root = tmp("queue");
    // The producer file also sends to a const-named kafka topic, so the LA.4
    // const fold rebuilds its queue nodes: the wrapper's node must survive.
    write(
        &root,
        "orders/publish.ts",
        "import { Kafka } from 'kafkajs';\n\nconst AUDIT_TOPIC = 'audit';\nconst producer = new Kafka({ brokers: [] }).producer();\n\nexport async function audit(o: unknown) {\n  await producer.send({ topic: AUDIT_TOPIC, messages: [] });\n}\n\nexport async function placeOrder(body: unknown) {\n  return publish(\"orders\", body);\n}\n",
    );
    write(
        &root,
        "orders/.glia/overlay.toml",
        "version = 1\n\n[[wrapper]]\ncall = \"publish\"\nkind = \"queue_producer\"\ntopic_arg = 0\nbroker = \"nats\"\n",
    );
    write(
        &root,
        "billing/worker.py",
        "def start():\n    subscribe(\"orders\", handle)\n\n\ndef handle(msg):\n    return msg\n",
    );
    write(
        &root,
        "billing/.glia/overlay.toml",
        "version = 1\n\n[[wrapper]]\ncall = \"subscribe\"\nkind = \"queue_consumer\"\ntopic_arg = 0\nlanguages = [\"python\"]\n",
    );
    let built = generate_many(&[s(&root.join("orders")), s(&root.join("billing"))]);
    std::fs::remove_dir_all(&root).ok();
    let r = built.expect("build");

    assert!(
        node(&r, "queue_producer:audit").is_some(),
        "the const fold ran"
    );
    let (p, kind) = node(&r, "queue_producer:orders").expect("wrapper producer");
    assert_eq!(kind, node_kind::QUEUE_PRODUCER);
    assert_eq!(p.confidence, Confidence::Weak);
    assert_eq!(
        json_cells(&p.cells, cell_type::ORIGIN),
        [serde_json::json!({"provenance": "overlay:llm", "rule": "wrapper#1"})]
    );
    let code = &json_cells(&p.cells, cell_type::CODE)[0];
    assert_eq!(
        (code["framework"].as_str(), code["family"].as_str()),
        (Some("nats"), Some("nats"))
    );
    assert_eq!(code["sites"][0]["line"], 10, "0-based site line");
    let pos = &json_cells(&p.cells, cell_type::POSITION)[0];
    assert_eq!(
        (pos["file"].as_str(), pos["start_line"].as_u64()),
        (Some("publish.ts"), Some(10))
    );
    assert!(
        has_edge(
            &r,
            "publish::placeOrder",
            "queue_producer:orders",
            edge_category::USES
        ),
        "{:?}",
        edges(&r, edge_category::USES)
    );
    assert!(has_edge(
        &r,
        "publish",
        "queue_producer:orders",
        edge_category::CONTAINS
    ));

    let (c, _) = node(&r, "queue_consumer:orders").expect("wrapper consumer");
    let code = &json_cells(&c.cells, cell_type::CODE)[0];
    assert_eq!(code["framework"], "wrapper");
    assert!(
        code.get("family").is_none(),
        "no broker: pairs with any family"
    );
    assert!(has_edge(
        &r,
        "queue_consumer:orders",
        "worker::start",
        edge_category::HANDLED_BY
    ));
    assert!(has_edge(
        &r,
        "queue_producer:orders",
        "queue_consumer:orders",
        edge_category::QUEUE_FLOWS
    ));
}

#[test]
fn no_overlay_mints_nothing() {
    let without_file = build_pair("none", CLIENT_TS, None, &BuildOptions::default());
    assert_eq!(endpoints(&without_file), ["endpoint:GET:<unresolved>"]);
    let switched_off = build_pair(
        "off",
        CLIENT_TS,
        Some(REQUEST_STANZA),
        &BuildOptions::default().with_overlay(false),
    );
    assert_eq!(
        endpoints(&switched_off),
        ["endpoint:GET:<unresolved>"],
        "--no-overlay"
    );
}

/// Map of file name -> bytes of a written layout.
fn store_bytes(r: &GenerateResult, dir: &Path) -> Vec<(String, Vec<u8>)> {
    write_merged_sharded(&r.merged, dir).expect("write store");
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .expect("read dir")
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).expect("read"),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn wrapper_builds_are_byte_identical() {
    let root = tmp("bytes");
    let repo = root.join("repo");
    let client = format!(
        "{CLIENT_TS}\nconst api = makeClient();\nexport async function listOrders() {{\n  return api.get('/orders');\n}}\n"
    );
    write(&repo, "web/src/client.ts", &client);
    write(&repo, "api/app.py", API_PY);
    write(
        &repo,
        ".glia/overlay.toml",
        &format!(
            "{REQUEST_STANZA}\n[[wrapper]]\ncall = \"api\"\nkind = \"http\"\nreceiver = true\n"
        ),
    );
    let path = s(&repo);
    let a = generate_one(&path).expect("build a");
    let b = generate_one(&path).expect("build b");
    assert!(
        endpoints(&a).contains(&"endpoint:GET:/orders".to_string()),
        "{:?}",
        endpoints(&a)
    );
    let bytes_a = store_bytes(&a, &root.join("a"));
    assert_eq!(bytes_a, store_bytes(&b, &root.join("b")), "clean vs clean");

    // Incremental: a warm cache serves the parse, the stage still runs post-cache.
    let mut cache = ParseCache::new();
    let _cold = generate_one_with_cache(&path, &mut cache).expect("cold");
    let warm = generate_one_with_cache(&path, &mut cache).expect("warm");
    assert!(cache.stats.reused > 0, "the parse came from the cache");
    assert_eq!(
        bytes_a,
        store_bytes(&warm, &root.join("warm")),
        "incremental vs clean"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn gaps_report_wrapped_sink_and_overlay_delta() {
    let root = tmp("gaps");
    let (web, api) = (root.join("web"), root.join("api"));
    write(&web, "src/client.ts", CLIENT_TS);
    write(&api, "app.py", API_PY);
    write(&web, ".glia/overlay.toml", REQUEST_STANZA);
    let paths = [s(&web), s(&api)];
    let r = generate_many(&paths).expect("build");
    let roots: Vec<(u64, PathBuf)> = r
        .repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect();
    let rep = gaps_report(&r.merged, &roots, &GapsOptions::default()).expect("report");
    let bare = gaps_report(&r.merged, &[], &GapsOptions::default()).expect("report");
    let delta = overlay_delta(&paths, false).expect("both builds");
    std::fs::remove_dir_all(&root).ok();

    // The wrapper's inner `fetch(path)` sink: informational once declared.
    assert_eq!(
        (rep.count(UNRESOLVED_ENDPOINT), rep.count(WRAPPED_SINK)),
        (0, 1),
        "{rep:#?}"
    );
    let row = rep
        .rows
        .iter()
        .find(|r| r.category == WRAPPED_SINK)
        .expect("row");
    assert_eq!(row.qname, "endpoint:GET:<unresolved>");
    assert_eq!(row.detail, "owner=src::client::request; wrapper=request");
    assert_eq!((row.suggest, row.tier), ("none", "fact"));
    // Without roots the overlay is unknown: still an unresolved endpoint.
    assert_eq!(bare.count(UNRESOLVED_ENDPOINT), 1);
    assert!(bare.skipped.contains(&WRAPPED_SINK));

    // Declaring the wrapper: +2 CALLS, +2 HTTP_CALLS, the sink leaves the orphans.
    assert_eq!(delta.rules, 1);
    assert_eq!(
        (delta.orphans_without, delta.orphans_with),
        (1, 0),
        "{delta:?}"
    );
    assert_eq!(delta.added_by_category.get("CALLS"), Some(&2), "{delta:?}");
    assert_eq!(
        delta.added_by_category.get("HTTP_CALLS"),
        Some(&2),
        "{delta:?}"
    );
}

/// The substrate-gap fixture `xcut-overlay-wrapper`, built here so its key
/// holds on this tree before the wheel `grade.py` reads is rebuilt.
#[test]
fn fixture_key_holds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/xcut-overlay-wrapper");
    let dirs = [s(&root.join("web")), s(&root.join("api"))];
    let r = generate_many(&dirs).expect("fixture builds");
    let eps = endpoints(&r);
    for q in [
        "endpoint:GET:/users",
        "endpoint:POST:/users",
        "endpoint:GET:/orders",
    ] {
        assert!(eps.contains(&q.to_string()), "{q}: {eps:?}");
    }
    assert!(
        !eps.contains(&"endpoint:GET:/legacy".to_string()),
        "commented out: {eps:?}"
    );
    assert!(has_edge(
        &r,
        "endpoint:GET:/users",
        "GET /users",
        edge_category::HTTP_CALLS
    ));
    assert!(has_edge(
        &r,
        "endpoint:POST:/users",
        "POST /users",
        edge_category::HTTP_CALLS
    ));
    assert!(has_edge(
        &r,
        "endpoint:GET:/orders",
        "GET /orders",
        edge_category::HTTP_CALLS
    ));
    assert!(has_edge(
        &r,
        "src::client::loadUsers",
        "endpoint:GET:/users",
        edge_category::CALLS
    ));
    assert!(has_edge(
        &r,
        "src::client::listOrders",
        "endpoint:GET:/orders",
        edge_category::CALLS
    ));
    let (n, _) = node(&r, "endpoint:GET:/orders").expect("node");
    assert_eq!(
        json_cells(&n.cells, cell_type::ORIGIN),
        [serde_json::json!({"provenance": "overlay:llm", "rule": "wrapper#2"})],
        "the receiver stanza is the second"
    );
}

// ---------------------------------------------------------------------------
// LG.3d: `kind = "data_entity"` — a project-local table / collection
// constructor mints the DATA_ENTITY the extractor would, function-level.
// Pre-fix baseline (HEAD dc961c0, fixture go-overlay-data-wrapper): the
// loader rejected the stanza (`unknown field flavor`, whole file ignored), so
// DATA_ENTITY 0/1, ACCESSES_DATA 0/1, ORIGIN 0/1.
// ---------------------------------------------------------------------------

const COLLECTION_GO: &str = "package repositories\n\nimport (\n\t\"go.mongodb.org/mongo-driver/mongo\"\n)\n\ntype Collection[T any] struct {\n\tinner *mongo.Collection\n}\n\nfunc NewCollection[T any](client *mongo.Client, database string, name string) *Collection[T] {\n\treturn &Collection[T]{inner: client.Database(database).Collection(name)}\n}\n";

const REPOSITORY_GO: &str = "package repositories\n\nimport (\n\t\"go.mongodb.org/mongo-driver/mongo\"\n)\n\ntype ChatPreview struct {\n\tRoomID string\n}\n\ntype ChatPreviewRepository struct {\n\tcollection *Collection[ChatPreview]\n}\n\nfunc NewChatPreviewRepository(client *mongo.Client, database string) *ChatPreviewRepository {\n\t// NewCollection[ChatPreview](client, database, \"legacy_previews\") was the old name.\n\treturn &ChatPreviewRepository{collection: NewCollection[ChatPreview](client, database, \"chat_previews\")}\n}\n";

const COLLECTION_STANZA: &str = "version = 1\n\n[[wrapper]]\ncall = \"NewCollection\"\nkind = \"data_entity\"\nflavor = \"nosql\"\nname_arg = 2\norigin = \"human\"\n";

/// One repo of `files`, built with `overlay` (when given) as its overlay.
fn build_repo(tag: &str, files: &[(&str, &str)], overlay: Option<&str>) -> GenerateResult {
    let root = tmp(tag);
    for (rel, body) in files {
        write(&root, rel, body);
    }
    if let Some(o) = overlay {
        write(&root, ".glia/overlay.toml", o);
    }
    let built = generate_one(&s(&root));
    std::fs::remove_dir_all(&root).ok();
    built.expect("build")
}

/// `(qname, name)` of every DATA_ENTITY node instance, sorted.
fn entities(r: &GenerateResult) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::DATA_ENTITY))
                .map(move |n| {
                    (
                        g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default(),
                        g.nav.name_by_id.get(&n.id).cloned().unwrap_or_default(),
                    )
                })
        })
        .collect();
    out.sort();
    out
}

/// The ACCESSES_DATA sources into `entity`, by qname, sorted.
fn accessors(r: &GenerateResult, entity: &str) -> Vec<String> {
    edges(r, edge_category::ACCESSES_DATA)
        .into_iter()
        .filter(|(_, t)| t == entity)
        .map(|(f, _)| f)
        .collect()
}

#[test]
fn data_entity_wrapper_mints_function_level_access() {
    // A direct driver call in a second file names the same collection: the
    // wrapper site and the extractor hit collapse onto one node.
    let direct = "package repositories\n\nimport (\n\t\"go.mongodb.org/mongo-driver/mongo\"\n)\n\nfunc CountPreviews(client *mongo.Client) {\n\tclient.Database(\"app\").Collection(\"chat_previews\")\n}\n";
    let r = build_repo(
        "entity",
        &[
            ("collection.go", COLLECTION_GO),
            ("chat_preview_repository.go", REPOSITORY_GO),
            ("count.go", direct),
        ],
        Some(COLLECTION_STANZA),
    );
    let q = "data_entity:nosql:chat_previews";
    assert_eq!(
        entities(&r),
        [(q.to_string(), "chat_previews".to_string())],
        "one node: the wrapper and the extractor agree on the qname; no legacy_previews / name / database"
    );
    let (n, kind) = node(&r, q).expect("node");
    assert_eq!(kind, node_kind::DATA_ENTITY);
    assert_eq!(
        json_cells(&n.cells, cell_type::ORIGIN),
        [serde_json::json!({"provenance": "overlay:human", "rule": "wrapper#1"})]
    );
    let pos = json_cells(&n.cells, cell_type::POSITION);
    assert!(
        pos.iter()
            .any(|p| p["file"] == "chat_preview_repository.go" && p["start_line"] == 16),
        "0-based line of the wrapper site: {pos:?}"
    );

    // Function-level: the wrapper edge leaves NewChatPreviewRepository, the
    // extractor's leaves CountPreviews; no module edge for either.
    let from = accessors(&r, q);
    assert_eq!(from.len(), 2, "{from:?}");
    assert!(from[0].ends_with("NewChatPreviewRepository"), "{from:?}");
    assert!(from[1].ends_with("CountPreviews"), "{from:?}");
    let wrapper_edge = r
        .merged
        .all_edges()
        .find(|e| {
            e.category == edge_category::ACCESSES_DATA
                && qname_of(&r, e.from).ends_with("NewChatPreviewRepository")
        })
        .expect("wrapper edge");
    assert_eq!(
        wrapper_edge.confidence,
        Confidence::Medium,
        "a human stanza"
    );
    let ev = Evidence::of(wrapper_edge).expect("EVIDENCE");
    assert_eq!(
        (
            ev.emitter.as_str(),
            ev.rule.as_deref(),
            ev.file.as_deref(),
            ev.line
        ),
        (
            "overlay:wrapper",
            Some("wrapper#1"),
            Some("chat_preview_repository.go"),
            Some(16)
        )
    );
}

#[test]
fn generic_brackets_do_not_hide_a_call() {
    let ts = "export function create<T>(db: unknown, schema: string, name: string): T {\n  return db as T;\n}\n\nexport function users() {\n  return create<User>(db, 'app', \"users\");\n}\n";
    let overlay = format!(
        "{COLLECTION_STANZA}\n[[wrapper]]\ncall = \"create\"\nkind = \"data_entity\"\nflavor = \"sql\"\nname_arg = 2\n"
    );
    let r = build_repo(
        "generic",
        &[
            ("collection.go", COLLECTION_GO),
            ("chat_preview_repository.go", REPOSITORY_GO),
            ("web/users.ts", ts),
        ],
        Some(&overlay),
    );
    let names: Vec<String> = entities(&r).into_iter().map(|(q, _)| q).collect();
    assert_eq!(
        names,
        ["data_entity:nosql:chat_previews", "data_entity:sql:users"],
        "`NewCollection[ChatPreview](` and `create<User>(` are both calls"
    );
    assert!(
        accessors(&r, "data_entity:sql:users")
            .iter()
            .any(|f| f.ends_with("users::users"))
    );
    let (n, _) = node(&r, "data_entity:sql:users").expect("node");
    assert_eq!(n.confidence, Confidence::Weak, "an llm stanza");
    assert_eq!(
        json_cells(&n.cells, cell_type::ORIGIN),
        [serde_json::json!({"provenance": "overlay:llm", "rule": "wrapper#2"})]
    );
}

#[test]
fn commented_and_definition_sites_mint_nothing() {
    // The definition `func NewCollection[T any](..., name string)` and the
    // commented `legacy_previews` call: only the live call mints.
    let r = build_repo(
        "entity-defs",
        &[
            ("collection.go", COLLECTION_GO),
            ("chat_preview_repository.go", REPOSITORY_GO),
        ],
        Some(COLLECTION_STANZA),
    );
    assert_eq!(
        entities(&r),
        [(
            "data_entity:nosql:chat_previews".to_string(),
            "chat_previews".to_string()
        )]
    );
    // Without the overlay, CA.4 infers the constructor (it hands its own
    // `name` to `.Collection(name)`): the same one entity, with the inferred
    // ORIGIN; the definition and the commented call still mint nothing.
    let off = build_repo(
        "entity-off",
        &[
            ("collection.go", COLLECTION_GO),
            ("chat_preview_repository.go", REPOSITORY_GO),
        ],
        None,
    );
    assert_inferred_chat_previews(&off);
}

/// CA.4: the COLLECTION_GO / REPOSITORY_GO pair with no applied stanza mints
/// exactly `data_entity:nosql:chat_previews`, through the inferred
/// `NewCollection` (defined at collection.go:11).
fn assert_inferred_chat_previews(r: &GenerateResult) {
    let q = "data_entity:nosql:chat_previews";
    assert_eq!(
        entities(r),
        [(q.to_string(), "chat_previews".to_string())],
        "no legacy_previews / name / database"
    );
    let (n, _) = node(r, q).expect("node");
    assert_eq!(
        json_cells(&n.cells, cell_type::ORIGIN),
        [serde_json::json!({
            "provenance": "inferred:wrapper",
            "rule": "inferred:NewCollection",
            "def": "collection.go:11"
        })]
    );
}

#[test]
fn nonliteral_name_is_skipped() {
    let named = "package repositories\n\nfunc NewNamedCollection[T any](client *mongo.Client, database string, name string) *Collection[T] {\n\treturn NewCollection[T](client, database, name)\n}\n\nfunc Concat(client *mongo.Client, database string, id string) {\n\tNewCollection[T](client, database, \"rooms_\"+id)\n}\n";
    let r = build_repo(
        "entity-nonlit",
        &[("collection.go", COLLECTION_GO), ("named.go", named)],
        Some(COLLECTION_STANZA),
    );
    assert!(entities(&r).is_empty(), "{:?}", entities(&r));
}

#[test]
fn missing_name_arg_is_a_config_error() {
    let overlay = "version = 1\n\n[[wrapper]]\ncall = \"NewCollection\"\nkind = \"data_entity\"\nflavor = \"nosql\"\n";
    let cfg = glia_code_domain::glia_config::parse_str(overlay);
    assert_eq!(cfg.errors.len(), 1, "{:?}", cfg.errors);
    assert!(
        cfg.errors[0].starts_with(".glia/overlay.toml:3: [[wrapper]]")
            && cfg.errors[0].contains("name_arg"),
        "{}",
        cfg.errors[0]
    );
    assert!(cfg.config.wrapper.is_empty(), "the stanza is dropped");
    // The dropped stanza applies nothing; CA.4's inference still reads the
    // constructor from the code.
    let r = build_repo(
        "entity-noarg",
        &[
            ("collection.go", COLLECTION_GO),
            ("chat_preview_repository.go", REPOSITORY_GO),
        ],
        Some(overlay),
    );
    assert_inferred_chat_previews(&r);
}

/// The substrate-gap fixture `go-overlay-data-wrapper`, built here so its
/// key holds on this tree before the wheel `grade.py` reads is rebuilt.
#[test]
fn data_fixture_key_holds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/go-overlay-data-wrapper");
    let r = generate_one(&s(&root)).expect("fixture builds");
    let q = "data_entity:nosql:chat_previews";
    assert_eq!(entities(&r), [(q.to_string(), "chat_previews".to_string())]);
    let from = accessors(&r, q);
    assert!(
        from.len() == 1 && from[0].ends_with("NewChatPreviewRepository"),
        "{from:?}"
    );
    let (n, _) = node(&r, q).expect("node");
    assert_eq!(
        json_cells(&n.cells, cell_type::ORIGIN),
        [serde_json::json!({"provenance": "overlay:human", "rule": "wrapper#1"})]
    );
}
