//! GH.18 — who starts this, through a real engine build of the probe stack.
//!
//! A Python service (`svc-py`: a module-scope worker loop, a Flask app, a
//! Celery app) started five ways (its Dockerfile's CMD, a compose service that
//! overrides the command, a k8s Deployment and CronJob, a Procfile, a systemd
//! unit), a Go, a Node and a Rust image. Before GH.18 no INFRA_RESOURCE had an
//! edge to code, no node carried an ENTRYPOINT cell
//! (`[live] .. declared=0 .. reached=6/48`) and blast radius from
//! `svc-py::worker::process` stopped at the worker MODULE, both dead.
//! After it each start is HANDLED_BY the code it runs, that code is an entry,
//! and the radius climbs to the image, the k8s workload, the systemd unit and
//! (over INFRA_REFERENCES) the compose service that overrides nothing.

use std::collections::BTreeSet;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category};
use glia_core::{CellPayload, NodeId};
use glia_engine::rules::{QueryOptions, QueryValue, query};
use glia_engine::{
    BlastOptions, blast_radius, coverage_report, entrypoint_reachable, generate_one,
};
use glia_graph::{MergedGraph, Reach};

const FILES: &[(&str, &str)] = &[
    (
        "svc-py/worker.py",
        "import time\n\n\ndef process(item):\n    return item * 2\n\n\ndef loop():\n    while True:\n        process(1)\n        time.sleep(1)\n\n\nloop()\n",
    ),
    (
        "svc-py/app.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/health\")\ndef health():\n    return \"ok\"\n",
    ),
    (
        "svc-py/tasks.py",
        "from celery import Celery\n\napp = Celery(\"tasks\")\n\n\n@app.task\ndef nightly():\n    return 1\n",
    ),
    (
        "svc-py/Dockerfile",
        "FROM python:3.12-slim\nWORKDIR /app\nCOPY . .\nCMD [\"python\", \"worker.py\"]\n",
    ),
    (
        "svc-py/Procfile",
        "web: gunicorn app:app\nworker: celery -A tasks worker\n",
    ),
    (
        "deploy/worker.service",
        "[Unit]\nDescription=worker\n\n[Service]\nWorkingDirectory=/srv/svc-py\nExecStart=/usr/bin/python3 /srv/svc-py/worker.py\n",
    ),
    ("svc-go/go.mod", "module example.com/svcgo\n\ngo 1.22\n"),
    (
        "svc-go/cmd/server/main.go",
        "package main\n\nimport \"net/http\"\n\nfunc main() {\n\thttp.ListenAndServe(\":8080\", nil)\n}\n",
    ),
    (
        "svc-go/Dockerfile",
        "FROM golang:1.22 AS build\nWORKDIR /src\nCOPY . .\nRUN go build -o /bin/server ./cmd/server\n\nFROM gcr.io/distroless/static\nCOPY --from=build /bin/server /bin/server\nENTRYPOINT [\"/bin/server\"]\n",
    ),
    (
        "svc-node/server.js",
        "const http = require(\"http\");\n\nfunction handle(req, res) {\n  res.end(\"ok\");\n}\n\nhttp.createServer(handle).listen(8080);\n",
    ),
    (
        "svc-node/Dockerfile",
        "FROM node:20\nWORKDIR /usr/src/app\nCOPY . .\nENTRYPOINT [\"node\", \"server.js\"]\n",
    ),
    (
        "svc-rust/Cargo.toml",
        "[package]\nname = \"svc-rust\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"api\"\npath = \"src/bin/api.rs\"\n",
    ),
    (
        "svc-rust/src/bin/api.rs",
        "fn main() {\n    println!(\"api\");\n}\n",
    ),
    (
        "svc-rust/Dockerfile",
        "FROM rust:1.80 AS build\nWORKDIR /app\nCOPY . .\nRUN cargo build --release --bin api\n\nFROM debian:bookworm-slim\nCOPY --from=build /app/target/release/api /usr/local/bin/api\nCMD [\"api\"]\n",
    ),
    (
        "docker-compose.yml",
        "services:\n  api:\n    build: ./svc-py\n    command: gunicorn app:app\n  worker:\n    build: ./svc-py\n  gosrv:\n    build: ./svc-go\n  app:\n    image: $APP\n    command: [\"$APP_CMD\"]\n",
    ),
    (
        "k8s/deploy.yaml",
        "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: py-worker\nspec:\n  template:\n    spec:\n      containers:\n        - name: worker\n          image: registry.example.com/svc-py:1.0\n          command: [\"python\"]\n          args: [\"worker.py\"]\n---\napiVersion: batch/v1\nkind: CronJob\nmetadata:\n  name: nightly\nspec:\n  schedule: \"0 3 * * *\"\n  jobTemplate:\n    spec:\n      template:\n        spec:\n          containers:\n            - name: nightly\n              image: svc-py\n              command: [\"python\", \"-m\", \"tasks\"]\n",
    ),
];

/// The stack, built; the tempdir is returned so it outlives the graph.
fn build() -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (rel, src) in FILES {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    (tmp, r.merged)
}

fn qname(merged: &MergedGraph, id: NodeId) -> String {
    merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

fn id_of(merged: &MergedGraph, q: &str) -> NodeId {
    merged
        .graphs
        .iter()
        .find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .find(|(_, v)| v.as_str() == q)
                .map(|(id, _)| *id)
        })
        .unwrap_or_else(|| panic!("no node {q}"))
}

#[test]
fn every_host_row_starts_its_code() {
    let (_tmp, merged) = build();
    let mut got: BTreeSet<(String, String, String)> = BTreeSet::new();
    for e in &merged.cross_edges {
        let Some(ev) = Evidence::of(e) else {
            continue;
        };
        if e.category != edge_category::HANDLED_BY || ev.emitter != "resolver:host" {
            continue;
        }
        let rule = ev.rule.unwrap_or_default();
        if rule == "manifest_bin" {
            continue;
        }
        got.insert((qname(&merged, e.from), qname(&merged, e.to), rule));
    }
    let want: BTreeSet<(String, String, String)> = [
        ("infra:cronjob:nightly", "svc-py::tasks", "k8s:py_module"),
        ("infra:deployment:py-worker", "svc-py::worker", "k8s:script"),
        (
            "infra:image:svc-go",
            "svc-go::cmd::server::main::main",
            "docker:binary",
        ),
        ("infra:image:svc-node", "svc-node::server", "docker:script"),
        ("infra:image:svc-py", "svc-py::worker", "docker:script"),
        (
            "infra:image:svc-rust",
            "svc-rust::src::bin::api::main",
            "docker:binary",
        ),
        (
            "infra:process:web@svc-py",
            "svc-py::app",
            "procfile:module_attr",
        ),
        (
            "infra:process:worker@svc-py",
            "svc-py::tasks",
            "procfile:celery_app",
        ),
        ("infra:service:api", "svc-py::app", "compose:module_attr"),
        ("infra:unit:worker", "svc-py::worker", "systemd:script"),
    ]
    .iter()
    .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
    .collect();
    assert_eq!(got, want);
}

#[test]
fn a_started_module_and_what_it_runs_are_live() {
    let (_tmp, merged) = build();
    let live = entrypoint_reachable(&merged);
    for q in [
        "svc-py::worker",
        "svc-py::worker::loop",
        "svc-py::worker::process",
        "svc-node::server",
        "svc-node::server::handle",
    ] {
        assert!(live.contains(&id_of(&merged, q)), "{q} is live");
    }
    // The ENTRYPOINT cell names the host that starts it.
    let worker = id_of(&merged, "svc-py::worker");
    let cells: Vec<String> = merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == worker)
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == cell_type::ENTRYPOINT)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        cells,
        [
            r#"{"decl":"svc-py/Dockerfile:4","source":"host:docker","host":"infra:image:svc-py","trigger":"container_start"}"#
        ]
    );
}

#[test]
fn blast_radius_climbs_to_the_hosts_that_start_it() {
    let (_tmp, merged) = build();
    let mut opts = BlastOptions::default();
    opts.direction = Reach::Both;
    opts.top_k = None;
    let answer = blast_radius(&merged, &["svc-py::worker::process"], &opts);
    let rows: BTreeSet<&str> = answer.results.iter().map(|r| r.qname.as_str()).collect();
    for q in [
        "svc-py::worker",
        "infra:image:svc-py",
        "infra:deployment:py-worker",
        "infra:unit:worker",
        "infra:service:worker",
    ] {
        assert!(rows.contains(q), "{q} in {rows:?}");
    }
}

/// One query's rows, each value as compared (a node's qname, a string).
fn rows(merged: &MergedGraph, text: &str) -> BTreeSet<Vec<String>> {
    let answer =
        query(merged, text, &QueryOptions::default()).unwrap_or_else(|e| panic!("{}", e.rendered));
    let shown = |v: &QueryValue| match v {
        QueryValue::Node { qname, .. } => qname.clone(),
        QueryValue::Str(s) => s.clone(),
        other => panic!("an unexpected value: {other:?}"),
    };
    answer
        .results
        .iter()
        .map(|r| r.values.iter().map(shown).collect())
        .collect()
}

#[test]
fn the_prelude_answers_who_starts_this() {
    let (_tmp, merged) = build();
    let starts = rows(&merged, "s(H, T) :- starts(H, T).");
    for (host, target) in [
        ("infra:image:svc-py", "svc-py::worker"),
        ("infra:service:api", "svc-py::app"),
        ("infra:deployment:py-worker", "svc-py::worker"),
        ("infra:cronjob:nightly", "svc-py::tasks"),
        ("infra:process:web@svc-py", "svc-py::app"),
        ("infra:unit:worker", "svc-py::worker"),
    ] {
        let row = vec![host.to_string(), target.to_string()];
        assert!(starts.contains(&row), "{row:?} in {starts:?}");
    }
    assert_eq!(starts.len(), 10, "{starts:?}");
    // When: the target's ENTRYPOINT cell (the first start of it).
    let when = rows(
        &merged,
        "w(T) :- starts(_, T), cell(T, \"ENTRYPOINT\", P), contains(P, '\"trigger\":\"schedule\"').",
    );
    assert_eq!(when, BTreeSet::from([vec!["svc-py::tasks".to_string()]]));
    let hosts = rows(
        &merged,
        "r(H) :- runs_in(N, H), qname(N, \"svc-py::worker::process\").",
    );
    let want: BTreeSet<Vec<String>> = [
        "infra:deployment:py-worker",
        "infra:image:svc-py",
        "infra:unit:worker",
    ]
    .iter()
    .map(|s| vec![s.to_string()])
    .collect();
    assert_eq!(hosts, want);
}

#[test]
fn a_host_repo_is_told_what_the_start_rules_cannot_see() {
    let (_tmp, merged) = build();
    let rows: Vec<_> = coverage_report(&merged)
        .into_iter()
        .filter(|n| n.language == "host")
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].edge_category, "HANDLED_BY");
    assert!(rows[0].edges_found > 0);
}
