//! A14.2 — `.kt` has its own parser, but Kotlin and Java build as ONE graph
//! (the JVM family, `build::lang_build`): a Kotlin file importing a Java class
//! resolves its IMPORTS edge into that class, and back, because resolution
//! only ever happens inside one `RepoGraph`. Before the flip both languages
//! were tagged `java`; building a separate `kotlin` graph would have severed
//! every cross-language edge of a mixed JVM repo.

use std::path::Path;

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::NodeKindId;
use repo_graph_engine::generate_one;
use repo_graph_graph::{MergedGraph, RepoGraph};

const JAVA_SERVICE: &str = "\
package com.acme.billing;

import com.acme.web.PriceFormat;

public class InvoiceService {
    public String total() {
        return PriceFormat.render(1);
    }
}
";

const KOTLIN_CONTROLLER: &str = "\
package com.acme.web

import com.acme.billing.InvoiceService

class InvoiceController(private val invoices: InvoiceService) {
    fun show(): String = invoices.total()
}

object PriceFormat {
    fun render(n: Int): String = n.toString()
}

fun helper(): Int = 1
";

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("jvm");
    for (rel, body) in files {
        write(&repo, rel, body);
    }
    let merged = generate_one(repo.to_str().unwrap())
        .unwrap_or_else(|e| panic!("generate_one jvm: {e}"))
        .merged;
    (tmp, merged)
}

/// The graphs holding a node of `kind` named `name`.
fn graph_with<'a>(merged: &'a MergedGraph, kind: NodeKindId, name: &str) -> Vec<&'a RepoGraph> {
    merged
        .graphs
        .iter()
        .filter(|g| {
            g.nav.kind_by_id.iter().any(|(id, k)| {
                *k == kind && g.nav.name_by_id.get(id).map(String::as_str) == Some(name)
            })
        })
        .collect()
}

/// `(from qname, to qname)` of every IMPORTS edge in `g` whose target is a
/// CLASS (a public class shares its qname with its file MODULE since LB.2).
fn class_imports(g: &RepoGraph) -> Vec<(&str, &str)> {
    g.edges
        .iter()
        .filter(|e| e.category == edge_category::IMPORTS)
        .filter(|e| g.nav.kind_by_id.get(&e.to) == Some(&node_kind::CLASS))
        .filter_map(|e| {
            Some((
                g.nav.qname_by_id.get(&e.from)?.as_str(),
                g.nav.qname_by_id.get(&e.to)?.as_str(),
            ))
        })
        .collect()
}

const JAVA_PATH: &str = "src/main/java/com/acme/billing/InvoiceService.java";
const KOTLIN_PATH: &str = "src/main/kotlin/com/acme/web/InvoiceController.kt";

#[test]
fn kotlin_and_java_share_one_graph_and_import_across_it() {
    let (_tmp, merged) = build(&[(JAVA_PATH, JAVA_SERVICE), (KOTLIN_PATH, KOTLIN_CONTROLLER)]);
    let java = graph_with(&merged, node_kind::CLASS, "InvoiceService");
    let kotlin = graph_with(&merged, node_kind::CLASS, "InvoiceController");
    assert_eq!(java.len(), 1, "one Java class node");
    assert_eq!(kotlin.len(), 1, "one Kotlin class node");
    assert!(
        std::ptr::eq(java[0], kotlin[0]),
        "Kotlin and Java build as one JVM graph"
    );
    let g = java[0];
    let edges = class_imports(g);
    // Kotlin -> Java: the Kotlin file module binds the Java class.
    assert!(
        edges.contains(&(
            "src::main::kotlin::com::acme::web::InvoiceController",
            "src::main::java::com::acme::billing::InvoiceService",
        )),
        "kotlin -> java IMPORTS: {edges:?}"
    );
    // Java -> Kotlin: the Java file module binds the Kotlin object.
    let price_format = g
        .nav
        .qname_by_id
        .iter()
        .find(|(id, q)| {
            q.as_str() == "src::main::kotlin::com::acme::web::PriceFormat"
                && g.nav.kind_by_id.get(id) == Some(&node_kind::CLASS)
        })
        .map(|(_, q)| q.as_str());
    assert!(price_format.is_some(), "the Kotlin object is a CLASS");
    assert!(
        edges.contains(&(
            "src::main::java::com::acme::billing::InvoiceService",
            "src::main::kotlin::com::acme::web::PriceFormat",
        )),
        "java -> kotlin IMPORTS: {edges:?}"
    );
    // The Kotlin parser's entities, with the LB.2 package scope on types.
    let qnames: Vec<&str> = g.nav.qname_by_id.values().map(String::as_str).collect();
    for want in [
        "src::main::kotlin::com::acme::web::InvoiceController::show",
        "src::main::kotlin::com::acme::web::PriceFormat::render",
        "src::main::kotlin::com::acme::web::InvoiceController::helper",
    ] {
        assert!(qnames.contains(&want), "missing {want}");
    }
}

#[test]
fn a_kotlin_only_repo_builds_its_own_graph() {
    let (_tmp, merged) = build(&[(KOTLIN_PATH, KOTLIN_CONTROLLER)]);
    let with_class = graph_with(&merged, node_kind::CLASS, "InvoiceController");
    assert_eq!(with_class.len(), 1);
    let fun = graph_with(&merged, node_kind::FUNCTION, "helper");
    assert_eq!(fun.len(), 1, "a top-level fun is a FUNCTION");
    let method = graph_with(&merged, node_kind::METHOD, "show");
    assert_eq!(method.len(), 1, "a member fun is a METHOD");
}

#[test]
fn a_java_only_repo_keeps_one_graph() {
    let (_tmp, merged) = build(&[(JAVA_PATH, JAVA_SERVICE)]);
    assert_eq!(merged.graphs.len(), 1, "no empty Kotlin graph beside Java");
    assert_eq!(graph_with(&merged, node_kind::CLASS, "InvoiceService").len(), 1);
}
