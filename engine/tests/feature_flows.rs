//! LG.3a — `feature_flows`: the per-feature flow records the dogfood repos'
//! `flows/<feature>.yaml` files are written from, and their writer.
//!
//! The fixture is `tests/fixtures/flows_stack`, two repos:
//! - `web` (Angular): `OrdersService.list` runs the literal
//!   `this.http.get('/api/orders')`, `OrdersComponent.load` calls it through the
//!   injected service, and `app.routes.ts` serves the page
//!   `{ path: 'orders', component: OrdersComponent }`;
//! - `api` (Go): gin `r.GET("/api/orders", ListOrders)`, whose body runs
//!   `db.Query("SELECT id FROM orders")`, and `events.go`
//!   `nc.Subscribe("orders.created", onCreated)`.
//!
//! Confirmed on the HEAD graph before any assertion was written
//! (`GLIA_NO_PERSIST=1 glia merge web api --out -`): `endpoint:GET:/api/orders
//! -HTTP_CALLS-> GET /api/orders` (the exact tier), `orders::ListOrders
//! -ACCESSES_DATA-> data_entity:sql:orders`, the module `orders`
//! `-ACCESSES_DATA-> data_source:database_sql`, and `page:/orders
//! -HANDLED_BY-> OrdersComponent -INJECTS-> OrdersService`.
//!
//! Before LG.3a the module slot held only its doc comment: none of the items
//! this file imports existed, so it did not compile.
//!
//! The fired_on marker is read from a child process: the
//! `child_flows_for_stderr` test re-runs this binary with `--nocapture` and
//! the parent reads its stderr.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::node_kind;
use glia_engine::feature_flows::{
    FeatureFlow, FlowEntry, FlowGrouping, FlowOptions, default_flows_dir, feature_flows,
    feature_key, render_flow_yaml, write_feature_flows,
};
use glia_engine::profile::CODE_PROFILE;
use glia_engine::trace::entry_flows;
use glia_engine::{GenerateResult, generate_many};
use glia_graph::Reach;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/flows_stack")
}

fn repo_paths() -> Vec<String> {
    let root = fixture();
    ["web", "api"]
        .iter()
        .map(|d| root.join(d).to_string_lossy().into_owned())
        .collect()
}

fn build() -> GenerateResult {
    generate_many(&repo_paths()).expect("generate_many over flows_stack")
}

fn flows_with(r: &GenerateResult, opts: &FlowOptions) -> Vec<FeatureFlow> {
    feature_flows(&r.merged, &r.repo_labels, opts)
}

fn feature<'a>(flows: &'a [FeatureFlow], key: &str) -> &'a FeatureFlow {
    flows.iter().find(|f| f.feature == key).unwrap_or_else(|| {
        let keys: Vec<&str> = flows.iter().map(|f| f.feature.as_str()).collect();
        panic!("no feature `{key}` in {keys:?}")
    })
}

fn entry<'a>(f: &'a FeatureFlow, kind: &str, qname: &str) -> &'a FlowEntry {
    f.entries
        .iter()
        .find(|e| e.entry.kind == kind && e.entry.qname == qname)
        .unwrap_or_else(|| {
            panic!(
                "no {kind} entry `{qname}` in feature {}: {:#?}",
                f.feature, f.entries
            )
        })
}

fn rank(c: &str) -> u8 {
    match c {
        "weak" => 0,
        "medium" => 1,
        "strong" => 2,
        other => panic!("unknown confidence `{other}`"),
    }
}

#[test]
fn orders_feature_joins_web_caller_api_handler_and_table() {
    let r = build();
    let flows = flows_with(&r, &FlowOptions::default());
    let orders = feature(&flows, "orders");

    assert_eq!(orders.services, ["api", "web"]);

    // The api ROUTE: the web's ENDPOINT calls it across the service boundary,
    // and the service method that holds the call is the next caller back.
    let api = entry(orders, "ROUTE", "GET /api/orders");
    assert_eq!(api.entry.service.as_deref(), Some("api"));
    assert_eq!((api.entry.depth, api.entry.via), (0, None));
    let endpoint = api
        .callers
        .iter()
        .find(|c| c.kind == "ENDPOINT")
        .unwrap_or_else(|| panic!("no ENDPOINT caller: {:#?}", api.callers));
    assert_eq!(endpoint.qname, "endpoint:GET:/api/orders");
    assert_eq!(endpoint.via, Some("HTTP_CALLS"));
    assert!(endpoint.cross_service, "{endpoint:#?}");
    assert_eq!(endpoint.service.as_deref(), Some("web"));
    assert_eq!(endpoint.depth, 1);
    let list = api
        .callers
        .iter()
        .find(|c| c.kind == "METHOD" && c.qname.ends_with("OrdersService::list"))
        .unwrap_or_else(|| panic!("no OrdersService::list caller: {:#?}", api.callers));
    assert_eq!(
        (list.via, list.depth, list.cross_service),
        (Some("CALLS"), 2, false)
    );
    // One hop further back (caller_depth 3): the component method calling it.
    assert!(
        api.callers
            .iter()
            .any(|c| c.depth == 3 && c.qname.ends_with("OrdersComponent::load")),
        "{:#?}",
        api.callers
    );
    // Callers come ordered by (depth, qname).
    let order: Vec<(usize, &str)> = api
        .callers
        .iter()
        .map(|c| (c.depth, c.qname.as_str()))
        .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);

    // The handler, then the table it queries.
    let handler = api
        .steps
        .iter()
        .find(|s| s.kind == "FUNCTION" && s.qname.ends_with("ListOrders"))
        .unwrap_or_else(|| panic!("no ListOrders step: {:#?}", api.steps));
    assert_eq!((handler.via, handler.depth), (Some("HANDLED_BY"), 1));
    assert_eq!(handler.file.as_deref(), Some("orders.go"));
    // 1-based: `func ListOrders` is line 11 of orders.go.
    assert_eq!(handler.line, Some(11));
    assert!(!handler.cross_service);
    assert!(
        api.steps
            .iter()
            .any(|s| s.qname == "data_entity:sql:orders" && s.via == Some("ACCESSES_DATA")),
        "{:#?}",
        api.steps
    );
    // `weakest` is the weakest confidence on the record.
    let min = api
        .callers
        .iter()
        .chain(&api.steps)
        .map(|s| rank(s.confidence))
        .min()
        .unwrap_or(2);
    assert_eq!(rank(api.weakest), min);

    // Data sources: the table ListOrders queries is a FACT (its own
    // function-level edge); the driver the module `orders` imports is a
    // HEURISTIC (the file holding the step touches it).
    let sinks: Vec<(&str, &str, &str, &str)> = orders
        .data_sources
        .iter()
        .map(|d| (d.qname.as_str(), d.tier, d.via, d.from.as_str()))
        .collect();
    assert_eq!(
        sinks,
        [
            (
                "data_entity:sql:orders",
                "FACT",
                "ACCESSES_DATA",
                "orders::ListOrders"
            ),
            ("data_source:database_sql", "HEURISTIC", "DEFINES", "orders"),
        ]
    );
    assert_eq!(orders.data_sources[0].kind, "DATA_ENTITY");

    // The client page `/orders` is an entry of the same feature. Since LA.6's
    // lift the page ROUTE is HANDLED_BY its component, so its flow is the
    // component and the service it injects (DEFINES is not a carry edge: the
    // flow stops at the class, LD.4b's hand-off).
    let page = entry(orders, "ROUTE", "page:/orders");
    assert_eq!(page.entry.service.as_deref(), Some("web"));
    assert!(page.callers.is_empty(), "{:#?}", page.callers);
    let steps: Vec<(usize, Option<&str>, &str)> = page
        .steps
        .iter()
        .map(|s| (s.depth, s.via, s.qname.as_str()))
        .collect();
    assert_eq!(
        steps,
        [
            (
                1,
                Some("HANDLED_BY"),
                "src::app::orders::orders.component::OrdersComponent"
            ),
            (
                2,
                Some("INJECTS"),
                "src::app::orders::orders.service::OrdersService"
            ),
        ]
    );
    // Entries in a feature come ordered by qname.
    let quals: Vec<&str> = orders
        .entries
        .iter()
        .map(|e| e.entry.qname.as_str())
        .collect();
    assert_eq!(quals, ["GET /api/orders", "page:/orders"]);
}

#[test]
fn queue_consumer_is_its_own_feature() {
    let r = build();
    let flows = flows_with(&r, &FlowOptions::default());
    let q = feature(&flows, "queue-orders.created");
    assert_eq!(q.entries.len(), 1);
    let e = entry(q, "QUEUE_CONSUMER", "queue_consumer:orders.created");
    assert!(
        e.steps
            .iter()
            .any(|s| s.qname == "events::onCreated" && s.via == Some("HANDLED_BY")),
        "{:#?}",
        e.steps
    );
    assert_eq!(q.services, ["api"]);
    assert!(q.data_sources.is_empty(), "{:#?}", q.data_sources);
    // Features come sorted by key; nothing but the three entries is a flow
    // entry (the component, `main` and the ENDPOINT are not).
    let keys: Vec<&str> = flows.iter().map(|f| f.feature.as_str()).collect();
    assert_eq!(keys, ["orders", "queue-orders.created"]);
    let entries: usize = flows.iter().map(|f| f.entries.len()).sum();
    assert_eq!(entries, 3);

    // `feature` narrows to one record; `scope` keeps the entries whose file
    // is under it (the api repo's `events.go` is outside `src`).
    let one = flows_with(
        &r,
        &FlowOptions::default().with_feature("queue-orders.created"),
    );
    assert_eq!(one.len(), 1);
    assert_eq!(one[0], *q);
    let scoped = flows_with(&r, &FlowOptions::default().with_scope("src"));
    let keys: Vec<&str> = scoped.iter().map(|f| f.feature.as_str()).collect();
    assert_eq!(keys, ["orders"]);
}

#[test]
fn entry_grouping_gives_one_record_per_entry() {
    let r = build();
    let by_entry = flows_with(
        &r,
        &FlowOptions::default().with_grouping(FlowGrouping::Entry),
    );
    let keys: Vec<&str> = by_entry.iter().map(|f| f.feature.as_str()).collect();
    assert_eq!(keys, ["get_-api-orders", "orders", "orders.created"]);
    assert!(
        by_entry.iter().all(|f| f.entries.len() == 1),
        "{by_entry:#?}"
    );

    // Two repos serving one route: one key each, suffixed in (key, qname)
    // order, never HashMap order.
    let td = tempfile::tempdir().expect("tempdir");
    for d in ["a", "b"] {
        let dir = td.path().join(d);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("go.mod"),
            format!("module example.com/{d}\n\ngo 1.22\n"),
        )
        .expect("go.mod");
        std::fs::write(
            dir.join("main.go"),
            "package main\n\nimport \"github.com/gin-gonic/gin\"\n\nfunc health(c *gin.Context) {}\n\nfunc main() {\n\tr := gin.Default()\n\tr.GET(\"/health\", health)\n}\n",
        )
        .expect("main.go");
    }
    let paths: Vec<String> = ["a", "b"]
        .iter()
        .map(|d| td.path().join(d).to_string_lossy().into_owned())
        .collect();
    let twin = generate_many(&paths).expect("generate_many a b");
    let flows = feature_flows(
        &twin.merged,
        &twin.repo_labels,
        &FlowOptions::default().with_grouping(FlowGrouping::Entry),
    );
    let keys: Vec<&str> = flows.iter().map(|f| f.feature.as_str()).collect();
    assert_eq!(keys, ["get_-health", "get_-health-2"]);
    let unique: BTreeSet<&str> = keys.iter().copied().collect();
    assert_eq!(unique.len(), keys.len());
    // Feature grouping folds both into one feature `health`.
    let grouped = feature_flows(&twin.merged, &twin.repo_labels, &FlowOptions::default());
    assert_eq!(grouped.len(), 1);
    assert_eq!(grouped[0].feature, "health");
    assert_eq!(grouped[0].entries.len(), 2);
    assert_eq!(grouped[0].services, ["a", "b"]);
}

#[test]
fn feature_key_rules() {
    use node_kind as nk;
    let route = |q: &str| feature_key(nk::ROUTE, q, q);
    assert_eq!(route("GET /api/v2/orders/:id"), "orders");
    assert_eq!(route("GET /"), "root");
    assert_eq!(feature_key(nk::ROUTE, "page:/", "/"), "root");
    assert_eq!(route("GET /:tenant/{x}"), "root");
    assert_eq!(route("GET /:tenant/orders"), "orders");
    assert_eq!(route("POST /API/V1/Users/<id>"), "users");
    assert_eq!(route("GET /api/[slug]/**"), "root");
    assert_eq!(route("GET /api/orders @services/api"), "orders");
    assert_eq!(feature_key(nk::ROUTE, "page:/orders", "/orders"), "orders");
    assert_eq!(
        feature_key(nk::ROUTE, "route:/files/*any", "/files/*any"),
        "files"
    );
    assert_eq!(
        feature_key(nk::WS_HANDLER, "ws:/orders/ws", "/orders/ws"),
        "orders"
    );
    assert_eq!(
        feature_key(
            nk::QUEUE_CONSUMER,
            "queue_consumer:orders.created",
            "orders.created"
        ),
        "queue-orders.created"
    );
    assert_eq!(
        feature_key(
            nk::QUEUE_CONSUMER,
            "queue_consumer:orders.created @svc/a",
            "orders.created"
        ),
        "queue-orders.created"
    );
    assert_eq!(
        feature_key(
            nk::EVENT_HANDLER,
            "event_handle:user:signed_up",
            "user:signed_up"
        ),
        "event-user-signed_up"
    );
    assert_eq!(
        feature_key(
            nk::GRPC_SERVICE,
            "grpc:shop.v1.OrderService",
            "OrderService"
        ),
        "grpc-orderservice"
    );
    assert_eq!(
        feature_key(nk::GRAPHQL_RESOLVER, "graphql_resolver:orders", "orders"),
        "graphql"
    );
    assert_eq!(
        feature_key(nk::CLI_COMMAND, "cli:deploy now", "deploy now"),
        "cli-deploy"
    );
    assert_eq!(
        feature_key(nk::CRON_JOB, "cron:0 * * * *:cleanup", "cleanup"),
        "cron-0-cleanup"
    );
    assert_eq!(
        feature_key(nk::RPC_PROCEDURE, "rpc:orders.Create", "orders.Create"),
        "rpc_procedure-orders.create"
    );

    // A 200-byte path mixing ASCII and multibyte characters: the key stays
    // within 64 bytes, is valid UTF-8 cut on a char boundary, and ends clean.
    let long = format!("GET /{}", "ordé".repeat(40));
    assert!(long.len() >= 200, "{}", long.len());
    let k = route(&long);
    assert!(k.len() <= 64, "{} bytes: {k}", k.len());
    assert!(
        !k.is_empty() && !k.ends_with('-') && !k.starts_with('-'),
        "{k}"
    );
    assert!(
        k.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b)),
        "{k}"
    );
    // Nothing but disallowed characters: a route falls to `root`.
    assert_eq!(route("GET /заказы"), "root");
}

#[test]
fn steps_equal_entry_flow_reach() {
    let r = build();
    let depth = FlowOptions::default().depth;
    let flows = flows_with(&r, &FlowOptions::default());
    let reach = entry_flows(&r.merged, &r.repo_labels, depth);
    let carry = CODE_PROFILE.tables.carry_edges;
    let mut checked = 0;
    for f in &flows {
        for e in &f.entries {
            let Some(ef) = reach
                .iter()
                .find(|ef| ef.entry.qname == e.entry.qname && ef.entry.kind == e.entry.kind)
            else {
                // LD.4b keeps only entries reaching >= 1 node.
                assert!(e.steps.is_empty(), "{e:#?}");
                continue;
            };
            assert_eq!(e.steps.len(), ef.reach, "{}", e.entry.qname);
            let id = r
                .merged
                .node_id_by_qname(&e.entry.qname)
                .unwrap_or_else(|| panic!("no node `{}`", e.entry.qname));
            let reached: BTreeMap<String, usize> = r
                .merged
                .bfs(&[id], Reach::Forward, Some(carry), depth)
                .iter()
                .map(|x| {
                    let q = r
                        .merged
                        .graphs
                        .iter()
                        .find_map(|g| g.nav.qname_by_id.get(&x.id).cloned())
                        .unwrap_or_default();
                    (q, x.depth)
                })
                .collect();
            for s in &e.steps {
                assert_eq!(
                    reached.get(&s.qname),
                    Some(&s.depth),
                    "{} -> {}",
                    e.entry.qname,
                    s.qname
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 3, "every fixture entry reaches something");
}

/// Every regular file under `dir` (one level), with its bytes.
fn files_in(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .expect("read flows dir")
        .map(|e| e.expect("dir entry"))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name, std::fs::read(e.path()).expect("read file"))
        })
        .collect()
}

#[test]
fn writer_is_byte_stable_and_prunes_only_its_own_files() {
    let r = build();
    let roots_owned = repo_paths();
    let roots: Vec<&Path> = roots_owned.iter().map(Path::new).collect();
    let out = tempfile::tempdir().expect("tempdir");
    let dir = out.path().join("flows");
    let all = flows_with(&r, &FlowOptions::default());
    let n = all.len();
    assert_eq!(n, 2);

    let first =
        write_feature_flows(&roots, &dir, &all, FlowGrouping::Feature).expect("first write");
    assert_eq!((first.written, first.unchanged, first.removed), (n, 0, 0));
    let before = files_in(&dir);
    let names: Vec<&str> = before.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        ["index.json", "orders.yaml", "queue-orders.created.yaml"]
    );
    let mtime = |f: &str| {
        std::fs::metadata(dir.join(f))
            .and_then(|m| m.modified())
            .expect("mtime")
    };
    let orders_mtime = mtime("orders.yaml");
    let index_mtime = mtime("index.json");

    // The YAML: a header, then one JSON record per line.
    let yaml = String::from_utf8(before["orders.yaml"].clone()).expect("utf-8");
    assert_eq!(yaml, render_flow_yaml(&all[0], FlowGrouping::Feature));
    let lines: Vec<&str> = yaml.lines().collect();
    assert!(lines[0].starts_with("# generated by glia "), "{}", lines[0]);
    assert!(
        lines[0].ends_with(" - glia flows; do not edit"),
        "{}",
        lines[0]
    );
    assert_eq!(lines[1], "feature: \"orders\"");
    assert_eq!(lines[2], "grouping: feature");
    assert_eq!(lines[3], "services: [\"api\",\"web\"]");
    assert!(
        yaml.contains("\n  - entry: {\"qname\":\"GET /api/orders\",\"kind\":\"ROUTE\","),
        "{yaml}"
    );
    assert!(
        yaml.contains("\n  - {\"qname\":\"data_entity:sql:orders\",\"kind\":\"DATA_ENTITY\",\"via\":\"ACCESSES_DATA\",\"tier\":\"FACT\",\"from\":\"orders::ListOrders\"}\n"),
        "{yaml}"
    );
    // Every record line is one JSON object.
    for l in yaml.lines().filter(|l| l.trim_start().starts_with("- {")) {
        let json = l.trim_start().trim_start_matches("- ");
        serde_json::from_str::<serde_json::Value>(json).unwrap_or_else(|e| panic!("{e}: {l}"));
    }

    let index: serde_json::Value =
        serde_json::from_slice(&before["index.json"]).expect("index.json is JSON");
    assert_eq!(index["grouping"], "feature");
    assert!(
        index["generator"]
            .as_str()
            .is_some_and(|g| g.starts_with("glia ")),
        "{index}"
    );
    let orders = &index["features"][0];
    assert_eq!(orders["feature"], "orders");
    assert_eq!(orders["file"], "orders.yaml");
    assert_eq!(orders["entries"], 2);
    assert_eq!(orders["data_sources"], 2);

    // Twice: nothing rewritten, not even the mtime.
    let second =
        write_feature_flows(&roots, &dir, &all, FlowGrouping::Feature).expect("second write");
    assert_eq!(
        (second.written, second.unchanged, second.removed),
        (0, n, 0)
    );
    assert_eq!(files_in(&dir), before);
    assert_eq!(mtime("orders.yaml"), orders_mtime);
    assert_eq!(mtime("index.json"), index_mtime);

    // A hand-placed file glia never listed survives every prune.
    std::fs::write(dir.join("notes.yaml"), "mine: true\n").expect("notes.yaml");
    let only = flows_with(&r, &FlowOptions::default().with_feature("orders"));
    assert_eq!(only.len(), 1);
    let third =
        write_feature_flows(&roots, &dir, &only, FlowGrouping::Feature).expect("third write");
    assert_eq!((third.written, third.unchanged, third.removed), (0, 1, 1));
    let names: Vec<String> = files_in(&dir).into_keys().collect();
    assert_eq!(names, ["index.json", "notes.yaml", "orders.yaml"]);

    // And back: the pruned feature is written again, nothing else moves.
    let fourth =
        write_feature_flows(&roots, &dir, &all, FlowGrouping::Feature).expect("fourth write");
    assert_eq!(
        (fourth.written, fourth.unchanged, fourth.removed),
        (1, 1, 0)
    );
    assert_eq!(
        std::fs::read(dir.join("notes.yaml")).expect("notes"),
        b"mine: true\n"
    );
}

#[test]
fn writer_refuses_a_walked_out_dir() {
    let r = build();
    let flows = flows_with(&r, &FlowOptions::default());
    let repo = tempfile::tempdir().expect("repo tempdir");
    std::fs::create_dir_all(repo.path().join("docs")).expect("docs");
    let roots = [repo.path()];

    for bad in [
        repo.path().join("docs/flows"),
        repo.path().join("flows"),
        repo.path().to_path_buf(),
        repo.path().join("not/yet/there"),
        repo.path().join(".glia/../docs/flows"),
    ] {
        let err = write_feature_flows(&roots, &bad, &flows, FlowGrouping::Feature)
            .expect_err("a walked dir is refused");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::InvalidInput,
            "{bad:?}: {err}"
        );
        assert!(
            err.to_string().contains(&bad.display().to_string()),
            "{err}"
        );
        assert!(!bad.join("index.json").exists(), "{bad:?}");
    }

    let layout = default_flows_dir(repo.path());
    assert_eq!(
        layout,
        repo.path().join(".glia").join("graph").join("flows")
    );
    let ok = write_feature_flows(&roots, &layout, &flows, FlowGrouping::Feature)
        .expect("the layout dir");
    assert_eq!(ok.written, flows.len());
    assert!(layout.join("orders.yaml").is_file());

    let elsewhere = tempfile::tempdir().expect("outside tempdir");
    let ok = write_feature_flows(&roots, elsewhere.path(), &flows, FlowGrouping::Feature)
        .expect("a dir outside every repo");
    assert_eq!(ok.written, flows.len());
}

const CHILD_ENV: &str = "GLIA_LG3A_CHILD_FLOWS";

/// The child half of [`flows_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it runs `feature_flows` over the fixture so
/// its stderr (with `--nocapture`) carries the marker.
#[test]
fn child_flows_for_stderr() {
    if std::env::var(CHILD_ENV).is_ok() {
        let r = build();
        flows_with(&r, &FlowOptions::default());
        flows_with(
            &r,
            &FlowOptions::default().with_grouping(FlowGrouping::Entry),
        );
    }
}

fn flows_stderr() -> Vec<String> {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "--exact",
            "child_flows_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .expect("re-run the test binary");
    assert!(
        out.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn marker_is_feature_flows() {
    let lines = flows_stderr();
    let ours: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|l| l.starts_with("[feature-flows] features="))
        .collect();
    assert_eq!(
        ours,
        [
            "[feature-flows] features=2 entries=3 steps=6 callers=3 sinks=2 (fact=1 heuristic=1) grouping=feature",
            "[feature-flows] features=3 entries=3 steps=6 callers=3 sinks=2 (fact=1 heuristic=1) grouping=entry",
        ]
    );
    // LD.4b owns `[flows]`: this packet never prints a `[flows] features=` line.
    assert!(
        !lines.iter().any(|l| l.starts_with("[flows] features=")),
        "{lines:#?}"
    );
}
