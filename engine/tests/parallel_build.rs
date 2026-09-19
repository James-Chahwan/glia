//! LG.1a: the per-file route / parse / extract runs on a rayon pool. The
//! pool's size must never reach the graph: every build below writes its
//! sharded store and compares BYTES, the way `byte_identical.rs` does.
//!
//! A test picks its pool size by running the build inside
//! `ThreadPoolBuilder::install`: the engine sees it is already on a rayon
//! worker and maps on that pool (`parallel::par_map_ordered`). A build called
//! from a plain thread uses the engine's own pool (`GLIA_THREADS`, default
//! every core); `cli/tests/parallel_cli.rs` covers `GLIA_THREADS=1`.

use std::collections::BTreeSet;
use std::path::Path;

use repo_graph_code_domain::node_kind;
use repo_graph_engine::{GenerateResult, ParseCache, generate_one, generate_one_with_cache};
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
