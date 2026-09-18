//! LB.2 — Java top-level types are package-scoped (`<dir>::Type`), not
//! file-scoped (`<dir>::<Stem>::Type`), through a real engine build.
//!
//! Before LB.2 the public class of `InvoiceService.java` was
//! `…::billing::InvoiceService::InvoiceService` and its method
//! `…::InvoiceService::InvoiceService::total`, so the qname a reader would type
//! found nothing and a JVM span / stack frame (`com.example.billing.
//! InvoiceService.total`) matched no qname suffix. Now the public class shares
//! its qname with its file MODULE; `pick_primary` puts the declaration first.
//!
//! Runs on a tempdir copy of the `java-package-qnames` fixture so the repo
//! identity is the copy's, not the glia checkout's.

use std::path::{Path, PathBuf};

use repo_graph_engine::{blast_radius_by_qname, generate_one, locate_node};
use repo_graph_graph::MergedGraph;

const MODULE: &str = "src::main::java::com::example::billing::InvoiceService";
const TOTAL: &str = "src::main::java::com::example::billing::InvoiceService::total";

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/java-package-qnames")
}

/// Copy the fixture's source tree (everything but the grader's `key.json`).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &dest);
        } else if entry.file_name() != "key.json" {
            std::fs::copy(&path, &dest).unwrap();
        }
    }
}

fn build() -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("java-package-qnames");
    copy_tree(&fixture_dir(), &repo);
    let merged = generate_one(repo.to_str().unwrap())
        .unwrap_or_else(|e| panic!("generate_one java-package-qnames: {e}"))
        .merged;
    (tmp, merged)
}

fn all_qnames(merged: &MergedGraph) -> Vec<&str> {
    let mut out: Vec<&str> = merged
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.values().map(|s| s.as_str()))
        .collect();
    out.sort_unstable();
    out
}

#[test]
fn no_qname_doubles_the_file_stem() {
    let (_tmp, merged) = build();
    let qnames = all_qnames(&merged);
    assert!(
        !qnames.iter().any(|q| q.contains("InvoiceService::InvoiceService")),
        "doubled class segment: {qnames:?}"
    );
    for want in [
        MODULE,
        TOTAL,
        "src::main::java::com::example::billing::InvoiceService::round",
        "src::main::java::com::example::billing::InvoiceService::Row",
        "src::main::java::com::example::billing::LineItem",
        "src::main::java::com::example::billing::LineItem::touch",
    ] {
        assert!(qnames.contains(&want), "missing {want}: {qnames:?}");
    }
}

#[test]
fn blast_radius_by_the_package_scoped_method_qname() {
    let (_tmp, merged) = build();
    let hits = blast_radius_by_qname(&merged, TOTAL, "forward", 4, None, false, None)
        .expect("the package-scoped method qname resolves");
    let round = hits
        .iter()
        .find(|h| h.qname == "src::main::java::com::example::billing::InvoiceService::round")
        .unwrap_or_else(|| {
            panic!(
                "total -> round in the radius: {:?}",
                hits.iter().map(|h| (&h.kind, &h.qname)).collect::<Vec<_>>()
            )
        });
    assert_eq!(round.kind, "METHOD");
    assert_eq!(round.reason, "CALLS");
}

#[test]
fn the_class_qname_resolves_to_the_class_not_its_file_module() {
    let (_tmp, merged) = build();
    let shared = merged.qnames_exact(MODULE);
    let mut kinds: Vec<&str> = shared.iter().map(|id| locate_node(&merged, *id).kind).collect();
    kinds.sort_unstable();
    assert_eq!(kinds, vec!["CLASS", "MODULE"], "the public class shares its file's qname");

    let id = merged.node_id_by_qname(MODULE).expect("qname resolves");
    let at = locate_node(&merged, id);
    assert_eq!(at.kind, "CLASS", "declaration beats the file module");
    assert_eq!(at.name, "InvoiceService");
    assert_eq!(
        at.file.as_deref(),
        Some("src/main/java/com/example/billing/InvoiceService.java")
    );
    assert!(at.line.is_some(), "the class is located");
}

#[test]
fn jvm_span_names_suffix_match_the_declarations() {
    let (_tmp, merged) = build();
    let method = merged
        .resolve_span("com.example.billing.InvoiceService.total")
        .expect("a JVM frame / span name suffix-matches the method qname");
    let at = locate_node(&merged, method);
    assert_eq!((at.kind, at.qname.as_str()), ("METHOD", TOTAL));

    let class = merged
        .resolve_span("com.example.billing.InvoiceService")
        .expect("the class span resolves");
    let at = locate_node(&merged, class);
    assert_eq!((at.kind, at.qname.as_str()), ("CLASS", MODULE));

    let nested = merged
        .resolve_span("com.example.billing.InvoiceService.Row")
        .expect("the nested class span resolves");
    assert_eq!(locate_node(&merged, nested).kind, "CLASS");
}
