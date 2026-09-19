//! LG.1a: the per-file route / parse / extract runs on a rayon pool. The
//! pool's size must never reach the graph: every build below writes its
//! sharded store and compares BYTES, the way `byte_identical.rs` does.
//!
//! A test picks its pool size by running the build inside
//! `ThreadPoolBuilder::install`: the engine sees it is already on a rayon
//! worker and maps on that pool (`parallel::par_map_ordered`). A build called
//! from a plain thread uses the engine's own pool (`GLIA_THREADS`, default
//! every core); `cli/tests/parallel_cli.rs` covers `GLIA_THREADS=1`.
//!
//! LG.1b: the walk's reads, the const-table scan, the RPC needle pass and the
//! multi-repo walks run on the same pool; the tests at the bottom pin them.

use std::collections::BTreeSet;
use std::path::Path;

use repo_graph_code_domain::node_kind;
use repo_graph_code_domain::walk_gating::repo_identity;
use repo_graph_core::{NodeKindId, RepoId};
use repo_graph_engine::{
    GenerateResult, ParseCache, generate_many, generate_one, generate_one_with_cache,
};
use repo_graph_store::write_merged_sharded;

/// Worker stack for the test pools: the engine pool's own size.
const STACK: usize = 16 << 20;

fn write(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// ~220 files, one or more per `route_one` branch: 40 Python, 40 Go (+ go.mod),
/// 40 TypeScript (an Angular HttpClient service and a component with its
/// `.component.html` template among them), 20 Rust (+ Cargo.toml), 20 Java,
/// a C/C++ header + source pair, a cross-language same-stem pair, and every
/// non-code branch: yaml (openapi, compose, k8s cron, feature list),
/// Dockerfile, dotenv, package manifest, migration `.sql`, Prisma, a sniffed
/// JSON contract and JSON Schema, `.proto`, `.graphql`, `.avsc`, plus a
/// README the docs pass reads.
fn write_fixture(dir: &Path) {
    for i in 0..40 {
        let j = (i + 1) % 40;
        write(
            dir,
            &format!("svc/py/mod_{i}.py"),
            &format!(
                "from svc.py.mod_{j} import f_{j}\n\n\ndef f_{i}(x):\n    \"\"\"Step {i}.\"\"\"\n    return f_{j}(x) + {i}\n"
            ),
        );
    }
    write(
        dir,
        "svc/py/app.py",
        "from flask import Flask\nfrom svc.py.mod_0 import f_0\n\napp = Flask(__name__)\n\n\n@app.route('/orders')\ndef list_orders():\n    return f_0(1)\n",
    );

    write(dir, "go.mod", "module example.com/fixture\n\ngo 1.22\n");
    for i in 0..40 {
        let j = (i + 1) % 40;
        write(
            dir,
            &format!("gosvc/g{i}.go"),
            &format!(
                "package gosvc\n\n// G{i} is step {i}.\nfunc G{i}(x int) int {{ return G{j}(x) + {i} }}\n"
            ),
        );
    }
    write(
        dir,
        "gosvc/server.go",
        "package gosvc\n\nimport \"net/http\"\n\nfunc Serve() {\n\thttp.HandleFunc(\"/orders\", func(w http.ResponseWriter, r *http.Request) { G0(1) })\n}\n",
    );

    for i in 0..38 {
        let j = (i + 1) % 38;
        write(
            dir,
            &format!("web/src/app/lib/m{i}.ts"),
            &format!(
                "import {{ t{j} }} from './m{j}';\n\nexport function t{i}(x: number): number {{\n  return t{j}(x) + {i};\n}}\n"
            ),
        );
    }
    write(
        dir,
        "web/src/app/orders/orders.service.ts",
        "import { Injectable } from '@angular/core';\nimport { HttpClient } from '@angular/common/http';\n\n@Injectable({ providedIn: 'root' })\nexport class OrdersService {\n  constructor(private http: HttpClient) {}\n\n  list() {\n    return this.http.get('/orders');\n  }\n}\n",
    );
    write(
        dir,
        "web/src/app/home/home.component.ts",
        "import { Component } from '@angular/core';\n\n@Component({ selector: 'app-home', templateUrl: './home.component.html' })\nexport class HomeComponent {}\n",
    );
    write(
        dir,
        "web/src/app/home/home.component.html",
        "<a routerLink=\"/orders\">orders</a>\n",
    );
    write(
        dir,
        "web/package.json",
        r#"{"name": "web", "dependencies": {"@angular/core": "17.0.0", "rxjs": "7.8.0"}}"#,
    );

    write(
        dir,
        "rs/Cargo.toml",
        "[package]\nname = \"fixture-rs\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1\"\n",
    );
    let mods: String = (0..19).map(|i| format!("pub mod m{i};\n")).collect();
    write(
        dir,
        "rs/src/lib.rs",
        &format!("{mods}\npub fn entry() -> i32 {{ m0::r0(1) }}\n"),
    );
    for i in 0..19 {
        let j = (i + 1) % 19;
        write(
            dir,
            &format!("rs/src/m{i}.rs"),
            &format!(
                "/// Step {i}.\npub fn r{i}(x: i32) -> i32 {{ crate::m{j}::r{j}(x) + {i} }}\n"
            ),
        );
    }

    for i in 0..20 {
        let j = (i + 1) % 20;
        write(
            dir,
            &format!("java/src/main/java/com/ex/C{i}.java"),
            &format!(
                "package com.ex;\n\npublic class C{i} {{\n    public int run(int x) {{\n        return new C{j}().run(x) + {i};\n    }}\n}}\n"
            ),
        );
    }

    write(dir, "native/widget.h", "int widget_spin(int x);\n");
    write(
        dir,
        "native/widget.cpp",
        "#include \"widget.h\"\n\nint widget_spin(int x) { return x + 1; }\n",
    );
    // LB.9b: one stem, two build groups: both MODULEs are named by file name.
    write(dir, "api/user.py", "def load_user(uid):\n    return uid\n");
    write(
        dir,
        "api/user.ts",
        "export function loadUser(uid: string): string {\n  return uid;\n}\n",
    );

    write(
        dir,
        "openapi.yaml",
        "openapi: 3.0.0\ninfo:\n  title: Orders\n  version: '1'\npaths:\n  /orders:\n    get:\n      operationId: listOrders\n      responses:\n        '200':\n          description: ok\n",
    );
    write(
        dir,
        "docker-compose.yml",
        "services:\n  api:\n    build: .\n    environment:\n      - DATABASE_URL=postgres://db/app\n    ports:\n      - \"8080:8080\"\n",
    );
    write(
        dir,
        "k8s/cron.yml",
        "apiVersion: batch/v1\nkind: CronJob\nmetadata:\n  name: nightly\nspec:\n  schedule: \"0 2 * * *\"\n",
    );
    write(
        dir,
        "features/orders/feature.yaml",
        "name: Orders\nbackend_routes:\n  protected:\n    - GET /orders\n",
    );
    write(
        dir,
        "Dockerfile",
        "FROM python:3.12-slim\nENV PORT=8080\nEXPOSE 8080\nCMD [\"python\", \"-m\", \"svc.py.app\"]\n",
    );
    write(
        dir,
        ".env.example",
        "PORT=8080\nDATABASE_URL=postgres://localhost/app\n",
    );
    write(
        dir,
        "db/migrations/V1__init.sql",
        "CREATE TABLE orders (id INT PRIMARY KEY);\nALTER TABLE orders ADD COLUMN total INT;\n",
    );
    write(
        dir,
        "prisma/schema.prisma",
        "datasource db {\n  provider = \"postgresql\"\n  url = env(\"DATABASE_URL\")\n}\n\nmodel Order {\n  id Int @id\n}\n",
    );
    write(
        dir,
        "docs/openapi.json",
        r#"{"openapi":"3.0.0","info":{"title":"Admin","version":"1"},"paths":{"/admin/orders":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#,
    );
    write(
        dir,
        "schemas/refund.schema.json",
        "{\n  \"$schema\": \"https://json-schema.org/draft/2020-12/schema\",\n  \"title\": \"Refund\",\n  \"type\": \"object\",\n  \"properties\": {\"id\": {\"type\": \"string\"}}\n}\n",
    );
    write(
        dir,
        "proto/api.proto",
        "syntax = \"proto3\";\npackage orders;\n\nservice OrderService {\n  rpc GetOrder (GetOrderRequest) returns (Order);\n}\n\nmessage GetOrderRequest { string id = 1; }\nmessage Order { string id = 1; int64 total = 2; }\n",
    );
    write(
        dir,
        "graphql/schema.graphql",
        "type Query {\n  order(id: ID!): Order\n}\n\ntype Order {\n  id: ID!\n}\n",
    );
    write(
        dir,
        "schemas/user.avsc",
        "{\"type\": \"record\", \"name\": \"User\", \"namespace\": \"com.ex\", \"fields\": [{\"name\": \"id\", \"type\": \"string\"}]}\n",
    );
    write(
        dir,
        "README.md",
        "# Fixture\n\nThe `OrdersService` calls `/orders`; `f_0` starts the chain.\n",
    );
}

/// The non-code MODULEs the fixture must mint, one per stashing branch, so a
/// branch `route_one` dropped fails here rather than passing unexercised.
const NON_CODE_MODULES: [&str; 14] = [
    "openapi.yaml",
    "docker-compose.yml",
    "k8s::cron.yml",
    "features::orders::feature.yaml",
    "Dockerfile",
    ".env.example",
    "web::package.json",
    "db::migrations::V1__init.sql",
    "prisma::schema.prisma",
    "docs::openapi.json",
    "schemas::refund.schema.json",
    "proto::api.proto",
    "graphql::schema.graphql",
    "schemas::user.avsc",
];

fn module_qnames(r: &GenerateResult) -> BTreeSet<String> {
    r.merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == node_kind::MODULE)
                .filter_map(|(id, _)| g.nav.qname_by_id.get(id).cloned())
        })
        .collect()
}

fn pool(n: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .stack_size(STACK)
        .build()
        .unwrap()
}

/// Map of file name -> bytes for every file in a sharded output dir.
fn dir_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn assert_dirs_byte_identical(a: &Path, b: &Path, context: &str) {
    let (fa, fb) = (dir_bytes(a), dir_bytes(b));
    let names = |fs: &[(String, Vec<u8>)]| fs.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(names(&fa), names(&fb), "{context}: file sets differ");
    for ((name, ba), (_, bb)) in fa.iter().zip(fb.iter()) {
        assert_eq!(ba, bb, "{context}: {name} bytes differ");
    }
}

#[test]
fn pool_size_does_not_change_a_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    write_fixture(&repo);
    let repo_s = repo.to_str().unwrap();

    let mut runs = Vec::new();
    for n in [1usize, 4, 16] {
        let r = pool(n).install(|| generate_one(repo_s)).unwrap();
        let out = tmp.path().join(format!("out_{n}"));
        write_merged_sharded(&r.merged, &out).unwrap();
        runs.push((n, r, out));
    }

    let (_, one, out_1) = &runs[0];
    let modules = module_qnames(one);
    for m in NON_CODE_MODULES {
        assert!(
            modules.contains(m),
            "the fixture must route {m}: {modules:?}"
        );
    }
    assert!(
        modules.contains("api::user.py") && modules.contains("api::user.ts"),
        "the LB.9b plan qualified the same-stem pair"
    );
    assert!(
        modules.contains("native::widget.h") && modules.contains("native::widget.cpp"),
        "the LB.10a plan names every C/C++ file by file name"
    );
    assert!(one.parse_errors.is_empty(), "{:?}", one.parse_errors);

    for (n, r, out) in &runs[1..] {
        assert_dirs_byte_identical(out_1, out, &format!("1 thread vs {n}"));
        assert_eq!(r.parse_errors, one.parse_errors, "{n} threads");
        assert_eq!(
            (r.total_nodes, r.total_edges),
            (one.total_nodes, one.total_edges),
            "{n} threads"
        );
    }

    // A build from a plain thread runs on the engine's own pool.
    let engine = generate_one(repo_s).unwrap();
    let out_engine = tmp.path().join("out_engine");
    write_merged_sharded(&engine.merged, &out_engine).unwrap();
    assert_dirs_byte_identical(out_1, &out_engine, "1 thread vs the engine pool");
}

#[test]
fn incremental_under_a_pool_matches_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    write_fixture(&repo);
    let repo_s = repo.to_str().unwrap();

    let mut cache = ParseCache::new();
    pool(4)
        .install(|| generate_one_with_cache(repo_s, &mut cache))
        .unwrap();
    for i in 0..10 {
        let j = (i + 1) % 40;
        write(
            &repo,
            &format!("svc/py/mod_{i}.py"),
            &format!(
                "from svc.py.mod_{j} import f_{j}\n\n\ndef f_{i}(x):\n    \"\"\"Step {i}, edited.\"\"\"\n    return f_{j}(x) * {i}\n"
            ),
        );
    }
    let incr = pool(16)
        .install(|| generate_one_with_cache(repo_s, &mut cache))
        .unwrap();
    assert!(
        cache.stats.reused > 0,
        "the rebuild reused cached parses: {:?}",
        cache.stats
    );
    assert_eq!(
        cache.stats.reparsed, 10,
        "exactly the edited files reparsed: {:?}",
        cache.stats
    );

    let clean = pool(1).install(|| generate_one(repo_s)).unwrap();
    let out_incr = tmp.path().join("out_incr");
    let out_clean = tmp.path().join("out_clean");
    write_merged_sharded(&incr.merged, &out_incr).unwrap();
    write_merged_sharded(&clean.merged, &out_clean).unwrap();
    assert_dirs_byte_identical(
        &out_incr,
        &out_clean,
        "16-thread incremental vs 1-thread clean",
    );
    assert_eq!(incr.parse_errors, clean.parse_errors);
}

/// Nesting depth of the generated Go file. Calibrated ONCE, 2026-09-19, on
/// HEAD 02f07e3 (debug build, the profile this test runs under):
/// `generate_one` on a `std::thread::Builder::stack_size(8 << 20)` thread (the
/// main-thread / CPython stack) returns Ok up to N = 4394 of these nested
/// calls and overflows at 4403; on a 2 MiB thread (cargo's test threads,
/// rayon's default worker) it overflows at 1074; on 16 MiB it holds 8834.
/// The overflow is in the Go parser's per-level recursion (parse_one alone
/// overflows at 4433 on 8 MiB). A TypeScript file of nested arrow functions,
/// the shape first planned, never overflowed at any depth: the TS visitors do
/// not recurse per level.
const DEEP_N: usize = 4394;

#[test]
fn deep_nesting_parses_on_worker_threads() {
    let tmp = tempfile::tempdir().unwrap();
    let mut src = String::from("package deep\n\nfunc G() int {\n\treturn ");
    src.push_str(&"h(".repeat(DEEP_N));
    src.push('0');
    src.push_str(&")".repeat(DEEP_N));
    src.push_str("\n}\n\nfunc h(x int) int { return x }\n");
    std::fs::write(tmp.path().join("deep.go"), src).unwrap();

    // From the 2 MiB test thread: the parse runs on the engine pool's 16 MiB
    // workers. Before LG.1a it ran here, and the process aborted.
    let r = generate_one(tmp.path().to_str().unwrap()).unwrap();
    assert!(r.parse_errors.is_empty(), "{:?}", r.parse_errors);
    assert!(
        module_qnames(&r).contains("deep"),
        "the file's MODULE is in the graph"
    );
}

/// Distinct nodes of `kind` across every graph (a marker id can sit in
/// several language graphs).
fn count_kind(r: &GenerateResult, kind: NodeKindId) -> usize {
    r.merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&kind))
                .map(|n| n.id.0)
        })
        .collect::<BTreeSet<_>>()
        .len()
}

const ORDERS_PROTO: &str = "syntax = \"proto3\";\npackage shop;\noption go_package = \"example.com/shop/pb\";\n\nservice Orders {\n  rpc Get (GetRequest) returns (Order);\n}\n\nmessage GetRequest { string id = 1; }\nmessage Order { string id = 1; }\n";

const ORDERS_SERVER: &str = "package main\n\nimport (\n\t\"context\"\n\n\t\"google.golang.org/grpc\"\n\tpb \"example.com/shop/pb\"\n)\n\ntype server struct {\n\tpb.UnimplementedOrdersServer\n}\n\nfunc (s *server) Get(ctx context.Context, in *pb.GetRequest) (*pb.Order, error) {\n\treturn &pb.Order{}, nil\n}\n\nfunc main() {\n\ts := grpc.NewServer()\n\tpb.RegisterOrdersServer(s, &server{})\n}\n";

/// A Go client repo: `pb.NewOrdersClient(conn)` names a service only the
/// server repo's `.proto` declares, so `apply_rpc_needles` mints its
/// GRPC_CLIENT from the build-wide service set; 30 more Go files give the
/// needle pass files to gate off.
fn write_orders_client(dir: &Path) {
    write(dir, "go.mod", "module example.com/client\n\ngo 1.22\n");
    write(
        dir,
        "main.go",
        "package main\n\nimport (\n\t\"context\"\n\n\t\"google.golang.org/grpc\"\n\tpb \"example.com/shop/pb\"\n)\n\nfunc main() {\n\tconn, _ := grpc.Dial(\"orders:50051\")\n\torders := pb.NewOrdersClient(conn)\n\t_, _ = orders.Get(context.Background(), &pb.GetRequest{})\n}\n",
    );
    for i in 0..30 {
        write(
            dir,
            &format!("util/u{i}.go"),
            &format!(
                "package util\n\n// U{i} is helper {i}.\nfunc U{i}(x int) int {{ return x + {i} }}\n"
            ),
        );
    }
}

/// A TS repo whose client URL is `${environment.apiUrl}/orders`: the const
/// table binds `apiUrl` and the A11.2 endpoint fold re-keys the ENDPOINT.
fn write_orders_web(dir: &Path) {
    write(
        dir,
        "src/environments/environment.ts",
        "export const environment = {\n  production: false,\n  apiUrl: 'http://orders.internal/api',\n};\n",
    );
    write(
        dir,
        "src/app/orders.service.ts",
        "import { Injectable } from '@angular/core';\nimport { HttpClient } from '@angular/common/http';\nimport { environment } from '../environments/environment';\n\n@Injectable({ providedIn: 'root' })\nexport class OrdersService {\n  constructor(private http: HttpClient) {}\n\n  list() {\n    return this.http.get(`${environment.apiUrl}/orders`);\n  }\n}\n",
    );
    for i in 0..30 {
        write(
            dir,
            &format!("src/app/lib/l{i}.ts"),
            &format!(
                "export const LABEL_{i} = 'label-{i}';\nexport function l{i}(x: number): number {{\n  return x + {i};\n}}\n"
            ),
        );
    }
}

/// The committed LA.17 Connect fixture: its Go and TS clients take the
/// Connect half of the needle pass (RPC_PROCEDURE / RPC_CALL).
fn connect_fixture_dirs() -> Vec<String> {
    let root = format!(
        "{}/../bench/substrate-gap/fixtures/xcut-connect-rpc",
        env!("CARGO_MANIFEST_DIR")
    );
    ["server", "client", "web"]
        .iter()
        .map(|d| format!("{root}/{d}"))
        .collect()
}

/// LG.1b: a multi-repo build walks its repos concurrently, scans the const
/// table and runs the RPC needle pass on the pool. None of it may reach the
/// store: 1 and 16 threads write the same bytes and report the same errors,
/// and every needle half (gRPC client, gRPC server, Connect) fired.
#[test]
fn multi_repo_pool_size_does_not_change_a_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let server = tmp.path().join("orders-server");
    write(&server, "api.proto", ORDERS_PROTO);
    write(&server, "main.go", ORDERS_SERVER);
    let client = tmp.path().join("orders-client");
    write_orders_client(&client);
    let web = tmp.path().join("orders-web");
    write_orders_web(&web);
    let mut repos: Vec<String> = [&server, &client, &web]
        .iter()
        .map(|p| p.to_str().unwrap().to_string())
        .collect();
    repos.extend(connect_fixture_dirs());

    let mut runs = Vec::new();
    for n in [1usize, 16] {
        let r = pool(n).install(|| generate_many(&repos)).unwrap();
        let out = tmp.path().join(format!("out_{n}"));
        write_merged_sharded(&r.merged, &out).unwrap();
        runs.push((n, r, out));
    }
    let (_, one, out_1) = &runs[0];
    assert!(one.parse_errors.is_empty(), "{:?}", one.parse_errors);
    let clients = count_kind(one, node_kind::GRPC_CLIENT);
    assert!(
        clients > 0,
        "the client repo's NewOrdersClient became a GRPC_CLIENT"
    );
    assert!(
        count_kind(one, node_kind::GRPC_SERVER) > 0,
        "the server half fired"
    );
    assert!(
        count_kind(one, node_kind::RPC_PROCEDURE) > 0,
        "the Connect half fired"
    );
    let endpoints: BTreeSet<String> = one
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == node_kind::ENDPOINT)
                .filter_map(|(id, _)| g.nav.qname_by_id.get(id).cloned())
        })
        .collect();
    assert!(
        endpoints
            .iter()
            .any(|q| q.contains("/orders") && !q.contains("${")),
        "the const table folded the environment base: {endpoints:?}"
    );

    let (_, sixteen, out_16) = &runs[1];
    assert_dirs_byte_identical(out_1, out_16, "multi-repo: 1 thread vs 16");
    assert_eq!(sixteen.parse_errors, one.parse_errors);
    assert_eq!(count_kind(sixteen, node_kind::GRPC_CLIENT), clients);
    assert_eq!(sixteen.repo_labels, one.repo_labels);
}

/// The distinct `g.repo` values over `merged.graphs`, in graph order: the
/// order that fixes shard indices.
fn repo_order(r: &GenerateResult) -> Vec<RepoId> {
    let mut out: Vec<RepoId> = Vec::new();
    for g in &r.merged.graphs {
        if out.last() != Some(&g.repo) {
            out.push(g.repo);
        }
    }
    out
}

fn repo_id_of(dir: &Path) -> RepoId {
    RepoId::from_canonical(&repo_identity(dir).key)
}

/// LG.1b: the phase-1 walks run concurrently but come back in argument
/// order. `alpha` holds 300 files and `beta` one, so a completion-order
/// collect would put `beta` first; a missing path keeps its error slot.
#[test]
fn walk_order_is_argument_order() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = tmp.path().join("alpha");
    for i in 0..300 {
        write(
            &alpha,
            &format!("pkg/m{i}.py"),
            &format!("def f_{i}(x):\n    return x + {i}\n"),
        );
    }
    let beta = tmp.path().join("beta");
    write(&beta, "main.py", "def main():\n    return 0\n");
    let (a, b) = (
        alpha.to_str().unwrap().to_string(),
        beta.to_str().unwrap().to_string(),
    );
    let missing = "/nonexistent-glia-lg1b".to_string();

    let r = pool(16)
        .install(|| generate_many(&[a.clone(), missing.clone(), b.clone()]))
        .unwrap();
    assert_eq!(
        r.parse_errors,
        vec!["not a directory: /nonexistent-glia-lg1b".to_string()]
    );
    assert_eq!(repo_order(&r), vec![repo_id_of(&alpha), repo_id_of(&beta)]);

    let rev = pool(16)
        .install(|| generate_many(&[b, missing, a]))
        .unwrap();
    assert_eq!(rev.parse_errors, r.parse_errors);
    assert_eq!(
        repo_order(&rev),
        vec![repo_id_of(&beta), repo_id_of(&alpha)]
    );
}

/// LG.1b: the walk reads on the pool, and a file its read rule rejects falls
/// through exactly as the inline reads did. A 600 KB README is no doc, a
/// 600 KB openapi.json is never sniffed, a 600 KB `Dockerfile.md` is no doc
/// but passes the source rule and routes as a Dockerfile, and non-UTF-8 files
/// are dropped. Same bytes on 1 and 16 threads.
#[test]
fn oversize_and_unreadable_files_fall_through_as_before() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let pad = "x".repeat(600_000);
    write(
        &repo,
        "README.md",
        &format!("# Big\n\nThe `serve` entry.\n\n{pad}\n"),
    );
    write(
        &repo,
        "docs/GUIDE.md",
        "# Guide\n\nCall `serve` to start.\n",
    );
    write(
        &repo,
        "api/openapi.json",
        &format!(
            r#"{{"openapi":"3.0.0","info":{{"title":"Big","version":"1"}},"paths":{{"/big":{{"get":{{}}}}}},"x-pad":"{pad}"}}"#
        ),
    );
    write(
        &repo,
        "api/small.json",
        r#"{"openapi":"3.0.0","info":{"title":"Small","version":"1"},"paths":{"/small":{"get":{"responses":{"200":{"description":"ok"}}}}}}"#,
    );
    write(
        &repo,
        "ops/Dockerfile.md",
        &format!("FROM python:3.12-slim\nEXPOSE 8080\n# {pad}\n"),
    );
    write(&repo, "app.py", "def serve():\n    return 0\n");
    std::fs::write(repo.join("broken.py"), b"def f():\n    return '\xff\xfe'\n").unwrap();
    std::fs::write(repo.join("docs/BROKEN.md"), b"# Broken \xff\n").unwrap();
    let repo_s = repo.to_str().unwrap();

    let one = pool(1).install(|| generate_one(repo_s)).unwrap();
    let sixteen = pool(16).install(|| generate_one(repo_s)).unwrap();
    let (out_1, out_16) = (tmp.path().join("out_1"), tmp.path().join("out_16"));
    write_merged_sharded(&one.merged, &out_1).unwrap();
    write_merged_sharded(&sixteen.merged, &out_16).unwrap();
    assert_dirs_byte_identical(&out_1, &out_16, "fall-through: 1 thread vs 16");
    assert_eq!(one.parse_errors, sixteen.parse_errors);

    let modules = module_qnames(&one);
    for m in ["app", "api::small.json", "ops::Dockerfile.md"] {
        assert!(modules.contains(m), "{m} is routed: {modules:?}");
    }
    for m in [
        "api::openapi.json",
        "broken",
        "README.md",
        "docs::BROKEN.md",
    ] {
        assert!(!modules.contains(m), "{m} is not routed: {modules:?}");
    }
    let doc_qnames: BTreeSet<String> = one
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == node_kind::DOC_SECTION)
                .filter_map(|(id, _)| g.nav.qname_by_id.get(id).cloned())
        })
        .collect();
    assert!(
        doc_qnames.iter().any(|q| q.contains("GUIDE")),
        "the small doc is ingested: {doc_qnames:?}"
    );
    assert!(
        !doc_qnames
            .iter()
            .any(|q| q.contains("README") || q.contains("BROKEN")),
        "the oversize and non-UTF-8 docs are not: {doc_qnames:?}"
    );
}
