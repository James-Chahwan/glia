//! HF.1 — declared entries seed entry flows. A node carrying an ENTRYPOINT
//! cell (overlay `[entrypoints]`, a Python `__main__` guard, a host start,
//! a host callback or plugin unit, a shebang / script_arg script) is a flow
//! entry, as it is already a liveness seed (`entrypoint_reachable`'s
//! `declared=`). Before HF.1 `trace::entries` read the entry table alone, so
//! on the process_entry shape below `glia flows` listed only `GET /health`
//! (`[flows] entries=1 flows=1`), `find process --flows` had `with_flows=0`
//! and `flows --concept loop` was a no_match absence although the container's
//! `python worker.py` runs `loop -> process`.
//!
//! The tree (bench/substrate-gap/patterns/python/process_entry, plus a
//! `__main__` script and an overlay declaration):
//! - `svc-py/Dockerfile` `CMD ["python", "worker.py"]` -> the `worker`
//!   MODULE (ENTRYPOINT `host:docker`), which calls `loop -> process`;
//! - `svc-py/docker-compose.yml` `command: gunicorn app:app` -> `app`
//!   (ENTRYPOINT `host:compose`), which reaches nothing over carry edges;
//! - `svc-py/report.py`, a `__main__` guard (ENTRYPOINT `main_guard`);
//! - `svc-py/jobs.py`, `nightly` and `main` declared in `[entrypoints]`
//!   (`main` is a table entry too: one flow, not two).

use std::path::Path;

use glia_engine::duplicate_flows::{DupFlowArgs, duplicate_flows};
use glia_engine::find::{FindOptions, find_flows};
use glia_engine::trace::{
    ConceptOptions, ConceptTier, DEFAULT_DEPTH, EntryFlow, concept_flows, entry_flows,
};
use glia_engine::{GenerateResult, generate_one};

const APP_PY: &str = "from flask import Flask

app = Flask(__name__)


@app.route(\"/health\")
def health():
    return \"ok\"
";

const WORKER_PY: &str = "import time


def process(item):
    return item * 2


def loop():
    while True:
        process(1)
        time.sleep(1)


loop()
";

const REPORT_PY: &str = "def summarize():
    return 1


if __name__ == \"__main__\":
    summarize()
";

const JOBS_PY: &str = "def cleanup():
    return 0


def nightly():
    cleanup()


def rotate():
    return 0


def main():
    rotate()
";

const COMPOSE: &str = "services:
  api:
    build: .
    command: gunicorn app:app
    ports:
      - \"8000:8000\"
";

const DOCKERFILE: &str = "FROM python:3.12-slim
WORKDIR /app
COPY . .
CMD [\"python\", \"worker.py\"]
";

fn write(root: &Path, rel: &str, src: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
    std::fs::write(p, src).expect("write source");
}

/// The tree above; `declare`: the `[entrypoints]` patterns of its overlay
/// (none: no overlay file).
fn build(declare: &[&str]) -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path().join("pe");
    write(&root, "svc-py/app.py", APP_PY);
    write(&root, "svc-py/worker.py", WORKER_PY);
    write(&root, "svc-py/report.py", REPORT_PY);
    write(&root, "svc-py/jobs.py", JOBS_PY);
    write(&root, "svc-py/docker-compose.yml", COMPOSE);
    write(&root, "svc-py/Dockerfile", DOCKERFILE);
    if !declare.is_empty() {
        let list: Vec<String> = declare.iter().map(|p| format!("{p:?}")).collect();
        write(
            &root,
            ".glia/overlay.toml",
            &format!(
                "version = 1\n\n[entrypoints]\nqnames = [{}]\n",
                list.join(", ")
            ),
        );
    }
    let r = generate_one(&root.to_string_lossy()).expect("generate_one");
    (td, r)
}

fn declared() -> (tempfile::TempDir, GenerateResult) {
    build(&["svc-py::jobs::nightly", "svc-py::jobs::main"])
}

fn flows(r: &GenerateResult) -> Vec<EntryFlow> {
    entry_flows(&r.merged, &r.repo_labels, DEFAULT_DEPTH)
}

fn flow<'a>(fs: &'a [EntryFlow], qname: &str) -> &'a EntryFlow {
    fs.iter()
        .find(|f| f.entry.qname == qname)
        .unwrap_or_else(|| {
            let got: Vec<&str> = fs.iter().map(|f| f.entry.qname.as_str()).collect();
            panic!("no flow entered at {qname}: {got:?}")
        })
}

fn hop_qnames(f: &EntryFlow) -> Vec<&str> {
    f.hops.iter().map(|h| h.to_qname.as_str()).collect()
}

#[test]
fn a_host_started_module_is_a_flow_entry() {
    let (_td, r) = build(&[]);
    let fs = flows(&r);
    let w = flow(&fs, "svc-py::worker");
    assert_eq!(w.key, "worker");
    assert_eq!(w.entry.kind, "MODULE");
    assert_eq!(w.entry.file.as_deref(), Some("svc-py/worker.py"));
    assert_eq!(
        hop_qnames(w),
        vec!["svc-py::worker::loop", "svc-py::worker::process"]
    );
    assert_eq!(w.reach, 2);
    // compose's `gunicorn app:app` declares app, which reaches nothing over
    // carry edges: no row.
    assert!(
        fs.iter().all(|f| !f.entry.qname.starts_with("svc-py::app")),
        "{:?}",
        fs.iter().map(|f| &f.entry.qname).collect::<Vec<_>>()
    );
    assert!(fs.iter().any(|f| f.entry.qname == "GET /health"));
}

#[test]
fn a_main_guard_script_is_a_flow_entry() {
    let (_td, r) = build(&[]);
    let fs = flows(&r);
    let s = flow(&fs, "svc-py::report");
    assert_eq!(s.entry.kind, "MODULE");
    assert_eq!(hop_qnames(s), vec!["svc-py::report::summarize"]);
}

#[test]
fn an_overlay_declared_function_is_a_flow_entry() {
    let (_td, r) = declared();
    let fs = flows(&r);
    let n = flow(&fs, "svc-py::jobs::nightly");
    assert_eq!(n.key, "nightly");
    assert_eq!(n.entry.kind, "FUNCTION");
    assert_eq!(hop_qnames(n), vec!["svc-py::jobs::cleanup"]);
    // Undeclared, nightly is no entry.
    let (_td2, plain) = build(&[]);
    assert!(
        flows(&plain)
            .iter()
            .all(|f| f.entry.qname != "svc-py::jobs::nightly")
    );
}

#[test]
fn a_table_entry_also_declared_is_one_flow() {
    let (_td, r) = declared();
    let fs = flows(&r);
    let mains: Vec<&EntryFlow> = fs
        .iter()
        .filter(|f| f.entry.qname == "svc-py::jobs::main")
        .collect();
    assert_eq!(mains.len(), 1, "one flow for a main that is also declared");
    assert_eq!(hop_qnames(mains[0]), vec!["svc-py::jobs::rotate"]);
    // Declaring it adds no row: it was a table entry already.
    let (_td2, plain) = build(&[]);
    assert_eq!(
        flows(&plain)
            .iter()
            .filter(|f| f.entry.qname == "svc-py::jobs::main")
            .count(),
        1
    );
}

#[test]
fn find_flows_sees_the_declared_flow() {
    let (_td, r) = build(&[]);
    let f = find_flows(&r.merged, "process", &FindOptions::default(), DEFAULT_DEPTH);
    let with_flows = f.flows.iter().filter(|t| !t.flows.is_empty()).count();
    assert_eq!(with_flows, 1, "{:?}", f.flows);
    let t = f
        .flows
        .iter()
        .find(|t| t.qname == "svc-py::worker::process")
        .expect("process is a hit");
    assert_eq!(t.flows[0].key, "worker");
    assert_eq!(t.flows[0].depth, 2);
}

#[test]
fn concept_search_reaches_the_declared_flow() {
    let (_td, r) = build(&[]);
    // `semantic: None` by default: the lexical tiers only.
    let opts = ConceptOptions::default();
    let a = concept_flows(&r.merged, &r.repo_labels, "loop", &opts);
    assert!(a.absence.is_none(), "{:?}", a.absence);
    assert_eq!(a.results.len(), 1, "{:?}", a.results);
    assert_eq!(a.results[0].key, "worker");
    assert_eq!(a.results[0].tier, ConceptTier::HopName);
}

#[test]
fn duplicate_flows_compares_the_declared_entries() {
    let (_td, r) = declared();
    let d = duplicate_flows(&r.merged, &r.repo_labels, &DupFlowArgs::default());
    // GET /health, worker, report, nightly, main (app reaches nothing but is
    // still an entry compared).
    let (_td2, plain) = build(&[]);
    let p = duplicate_flows(&plain.merged, &plain.repo_labels, &DupFlowArgs::default());
    assert_eq!(d.entries, p.entries + 1, "nightly is the one new entry");
    assert!(
        p.entries >= 5,
        "health, worker, app, report, main: {}",
        p.entries
    );
}
