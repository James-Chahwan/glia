//! DG.7b acceptance: an akka-quartz-scheduler job started from code by a
//! schedule NAME (`QuartzSchedulerExtension(system).schedule("Nightly", ..)`)
//! whose cron expression lives in `application.conf`
//! (`akka.quartz.schedules.Nightly.expression`) is the CRON_JOB
//! `cron:0 0 3 * * ?:Nightly`, SCHEDULED by the code file's MODULE.
//!
//! The expression is in another file than the call, so the per-file pass mints
//! a REF and the post-cache graft (`build::grafts`) re-keys it over the walked
//! confs. These tests pin the cache rule: a warm build equals a clean one, and
//! editing ONLY the conf re-keys the cache-served job. Before DG.7b the walk
//! never read the conf and the repo held no CRON_JOB at all. `grade.py` reads
//! the installed wheel, so these tests build from the working tree. The
//! `[cron] akka-quartz` fired_on marker is read from a child process
//! (`child_build_for_stderr`, the `event_const_names.rs` way).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use glia_code_domain::{edge_category, node_kind};
use glia_core::{EdgeCategoryId, NodeId};
use glia_engine::{GenerateResult, ParseCache, generate_one, generate_one_with_cache};
use glia_graph::MergedGraph;

const JOBS: &str = r#"package shop

import akka.actor.{ActorRef, ActorSystem}
import com.typesafe.akka.extension.quartz.QuartzSchedulerExtension

object Jobs {
  def start(system: ActorSystem, purger: ActorRef): Unit =
    QuartzSchedulerExtension(system).schedule("Nightly", purger, "purge")
}
"#;

const CONF_PATH: &str = "src/main/resources/application.conf";

fn conf(expression: &str) -> String {
    format!("akka.quartz.schedules {{\n  Nightly {{\n    expression = \"{expression}\"\n  }}\n}}\n")
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn build(dir: &Path) -> GenerateResult {
    generate_one(dir.to_str().unwrap()).expect("generate_one")
}

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// Every CRON_JOB qname, sorted.
fn cron_jobs(m: &MergedGraph) -> Vec<String> {
    let set: BTreeSet<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter_map(move |n| {
                (g.nav.kind_by_id.get(&n.id) == Some(&node_kind::CRON_JOB))
                    .then(|| g.nav.qname_by_id.get(&n.id).cloned())
                    .flatten()
            })
        })
        .collect();
    set.into_iter().collect()
}

/// `(from qname, to qname)` for every edge of `category`, sorted.
fn edges(m: &MergedGraph, category: EdgeCategoryId) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = m
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (qname(m, e.from), qname(m, e.to)))
        .collect();
    out.sort();
    out
}

/// Every node as `(id, qname)` and every edge as `(from, to, category)`.
type Shape = (BTreeSet<(u64, String)>, BTreeSet<(u64, u64, u32)>);

fn shape(m: &MergedGraph) -> Shape {
    let nodes = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .map(move |n| (n.id.0, g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default()))
        })
        .collect();
    let edges = m.all_edges().map(|e| (e.from.0, e.to.0, e.category.0)).collect();
    (nodes, edges)
}

/// The `.gmap` shard bytes of one build, by file name.
fn gmap_bytes(r: &GenerateResult, out: &Path) -> Vec<(String, Vec<u8>)> {
    glia_store::write_merged_sharded(&r.merged, out).expect("write .gmap");
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(out)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
        .collect();
    files.sort();
    files
}

fn s(a: &str) -> String {
    a.to_string()
}

fn pair(a: &str, b: &str) -> (String, String) {
    (s(a), s(b))
}

const CHILD_ENV: &str = "GLIA_DG7B_CHILD_BUILD";

/// The child half of [`marker`]: a no-op unless the parent set [`CHILD_ENV`].
#[test]
fn child_build_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        build(Path::new(&dir));
    }
}

/// The build's `[cron] akka-quartz` lines (without ` repo=<label>`) and its
/// per-file `[cron] code` lines.
fn markers(dir: &Path) -> Vec<String> {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_build_for_stderr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, dir)
        .output()
        .expect("re-run the test binary");
    assert!(out.status.success(), "child build failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("[cron] akka-quartz") || l.starts_with("[cron] code"))
        .map(|l| l.split(" repo=").next().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn conf_schedule_names_the_code_started_job() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "Jobs.scala", JOBS);
    write(td.path(), CONF_PATH, &conf("0 0 3 * * ?"));
    let r = build(td.path());
    assert_eq!(cron_jobs(&r.merged), [s("cron:0 0 3 * * ?:Nightly")]);
    assert_eq!(
        edges(&r.merged, edge_category::SCHEDULES),
        [pair("Jobs", "cron:0 0 3 * * ?:Nightly")]
    );
    assert_eq!(
        markers(td.path()),
        [
            s("[cron] code jobs=1 quartz=0 hangfire=0 go=0 apscheduler=0 spring=0 nestjs=0 tokio=0 akka_quartz=1 handler_refs=0 path=Jobs.scala"),
            s("[cron] akka-quartz confs=1 schedules=1 rekeyed=1 unresolved=0 conflicts=0"),
        ]
    );
}

#[test]
fn no_conf_no_job() {
    // The schedule is named nowhere: the REF is dropped, never shipped.
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "Jobs.scala", JOBS);
    let r = build(td.path());
    assert!(cron_jobs(&r.merged).is_empty(), "{:?}", cron_jobs(&r.merged));
    assert!(edges(&r.merged, edge_category::SCHEDULES).is_empty());
    assert_eq!(
        markers(td.path()).last().map(String::as_str),
        Some("[cron] akka-quartz confs=0 schedules=0 rekeyed=0 unresolved=1 conflicts=0")
    );
}

#[test]
fn incremental_equals_clean_and_a_conf_edit_rekeys() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("repo");
    write(&root, "Jobs.scala", JOBS);
    write(&root, CONF_PATH, &conf("0 0 3 * * ?"));
    let path = root.to_str().unwrap();

    let mut cache = ParseCache::new();
    let cold = generate_one_with_cache(path, &mut cache).expect("cold");
    let warm = generate_one_with_cache(path, &mut cache).expect("warm");
    let clean = generate_one(path).expect("clean");
    for r in [&cold, &warm] {
        assert_eq!(cron_jobs(&r.merged), [s("cron:0 0 3 * * ?:Nightly")]);
        assert_eq!(
            edges(&r.merged, edge_category::SCHEDULES),
            [pair("Jobs", "cron:0 0 3 * * ?:Nightly")]
        );
        assert_eq!(shape(&r.merged), shape(&clean.merged));
    }
    assert!(cache.last_diff().expect("a diff").reused.contains(&s("Jobs.scala")));

    // Edit ONLY the conf: the cache serves Jobs.scala, which still holds the
    // REF, and the graft re-keys it to the new expression.
    write(&root, CONF_PATH, &conf("0 30 4 * * ?"));
    let edited = generate_one_with_cache(path, &mut cache).expect("edited");
    let diff = cache.last_diff().expect("a diff").clone();
    assert!(diff.reused.contains(&s("Jobs.scala")), "{diff:?}");
    assert!(!diff.reparsed.contains(&s("Jobs.scala")), "{diff:?}");
    let cached = cache.iter().find(|c| c.path == "Jobs.scala").expect("cached parse");
    assert!(
        cached.parse.nav.qname_by_id.values().any(|q| q == "cron:@quartz-conf:Nightly:Nightly"),
        "the parse cache keeps the per-file REF"
    );
    assert_eq!(cron_jobs(&edited.merged), [s("cron:0 30 4 * * ?:Nightly")]);
    assert_eq!(
        edges(&edited.merged, edge_category::SCHEDULES),
        [pair("Jobs", "cron:0 30 4 * * ?:Nightly")]
    );
    let clean = generate_one(path).expect("clean after edit");
    assert_eq!(
        gmap_bytes(&edited, &td.path().join("warm")),
        gmap_bytes(&clean, &td.path().join("clean")),
        "incremental == clean"
    );

    // Dropping the conf drops the job on the next warm build.
    std::fs::remove_file(root.join(CONF_PATH)).unwrap();
    let gone = generate_one_with_cache(path, &mut cache).expect("no conf");
    assert!(cron_jobs(&gone.merged).is_empty(), "{:?}", cron_jobs(&gone.merged));
}
