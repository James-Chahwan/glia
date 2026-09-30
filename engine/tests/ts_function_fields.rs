//! CG.1: a TypeScript class field whose value is a function (`private onResize
//! = (): void => {…}`) is a METHOD of its class, end to end through
//! `generate_one`: its body's calls are CALLS edges from it carrying the
//! call's own site line, a client HTTP call in its body is its CALLS into the
//! ENDPOINT, and minting it moves no other method's liveness.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::{entrypoint_reachable, generate_one};
use glia_graph::MergedGraph;

const PACKAGE_JSON: &str = r#"{"name": "hero-app", "version": "1.0.0", "dependencies": {"@angular/core": "^17.0.0", "@angular/common": "^17.0.0"}}
"#;

/// The component, `{FIELD}` standing for the `onResize` field's value.
const HERO: &str = "\
import { Component } from '@angular/core';
import { HttpClient } from '@angular/common/http';

@Component({ selector: 'app-hero', template: '<div></div>' })
export class HeroComponent {
  constructor(private http: HttpClient) {}

  ngOnInit(): void {
    this.startAnimation();
  }

  startAnimation(): void {}

  stopAnimation(): void {}

  private onResize = {FIELD};
}
";

/// The function-valued field: `this.stopAnimation()` two rows below the
/// declaration, then a client HTTP call.
const ARROW: &str = "(): void => {
    const width = window.innerWidth;
    this.stopAnimation();
    this.http.get('/api/profile');
  }";

const PREFIX: &str = "src::hero.component::HeroComponent";

fn build(field: &str) -> (tempfile::TempDir, std::path::PathBuf, MergedGraph) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let files = [
        ("package.json", PACKAGE_JSON.to_string()),
        ("src/hero.component.ts", HERO.replace("{FIELD}", field)),
    ];
    for (rel, src) in &files {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, src).unwrap();
    }
    let merged = generate_one(repo.to_str().unwrap()).unwrap().merged;
    (tmp, repo, merged)
}

/// qname -> (id, kind) over every graph.
fn by_qname(m: &MergedGraph) -> HashMap<String, (NodeId, NodeKindId)> {
    let mut out = HashMap::new();
    for g in &m.graphs {
        for n in &g.nodes {
            let (Some(q), Some(k)) = (g.nav.qname_by_id.get(&n.id), g.nav.kind_by_id.get(&n.id))
            else {
                continue;
            };
            out.entry(q.clone()).or_insert((n.id, *k));
        }
    }
    out
}

fn method(nodes: &HashMap<String, (NodeId, NodeKindId)>, name: &str) -> NodeId {
    let q = format!("{PREFIX}::{name}");
    match nodes.get(&q) {
        Some((id, k)) if *k == node_kind::METHOD => *id,
        other => panic!(
            "no METHOD {q}: {other:?}; qnames: {:?}",
            nodes.keys().collect::<Vec<_>>()
        ),
    }
}

fn source_line(repo: &Path, file: &str, line: u32) -> String {
    let text = std::fs::read_to_string(repo.join(file)).unwrap();
    text.lines()
        .nth(line as usize)
        .unwrap_or_default()
        .to_string()
}

#[test]
fn arrow_field_is_a_method_with_located_calls() {
    let (_tmp, repo, m) = build(ARROW);
    let nodes = by_qname(&m);
    let on_resize = method(&nodes, "onResize");
    let stop = method(&nodes, "stopAnimation");

    let edge = m
        .all_edges()
        .find(|e| e.from == on_resize && e.to == stop && e.category == edge_category::CALLS)
        .expect("onResize CALLS stopAnimation");
    let ev = Evidence::of(edge).expect("the CALLS edge carries EVIDENCE");
    assert_eq!(ev.basis, Basis::Site, "{ev:?}");
    let (Some(file), Some(line)) = (ev.file.clone(), ev.line) else {
        panic!("site evidence without a file and line: {ev:?}");
    };
    let text = source_line(&repo, &file, line);
    assert!(
        text.contains("stopAnimation()"),
        "{file}:{line} is `{}`",
        text.trim()
    );

    let class = nodes
        .get(PREFIX)
        .map(|(id, _)| *id)
        .expect("the component CLASS");
    assert!(
        m.all_edges()
            .any(|e| e.from == class && e.to == on_resize && e.category == edge_category::DEFINES),
        "the class DEFINES its field METHOD"
    );
    assert!(
        !m.all_edges()
            .any(|e| e.from == class && e.to == stop && e.category == edge_category::CALLS),
        "the class takes none of the field body's calls"
    );
}

#[test]
fn http_call_in_an_arrow_field_is_the_fields() {
    let (_tmp, _repo, m) = build(ARROW);
    let nodes = by_qname(&m);
    let on_resize = method(&nodes, "onResize");
    let (endpoint, kind) = *nodes
        .get("endpoint:GET:/api/profile")
        .expect("the client ENDPOINT");
    assert_eq!(kind, node_kind::ENDPOINT);
    let froms: HashSet<NodeId> = m
        .all_edges()
        .filter(|e| e.to == endpoint && e.category == edge_category::CALLS)
        .map(|e| e.from)
        .collect();
    assert_eq!(
        froms,
        HashSet::from([on_resize]),
        "only onResize calls the ENDPOINT"
    );
}

/// Minting the field METHOD changes no other method's liveness: the same
/// component with a data field in its place reaches the same methods.
#[test]
fn other_methods_keep_their_liveness() {
    let (_t1, _r1, with_fn) = build(ARROW);
    let (_t2, _r2, with_data) = build("0");
    let (fn_nodes, data_nodes) = (by_qname(&with_fn), by_qname(&with_data));
    assert!(
        !data_nodes.contains_key(&format!("{PREFIX}::onResize")),
        "a data field is no METHOD"
    );
    let (fn_live, data_live) = (
        entrypoint_reachable(&with_fn),
        entrypoint_reachable(&with_data),
    );
    for name in ["constructor", "ngOnInit", "startAnimation", "stopAnimation"] {
        assert_eq!(
            fn_live.contains(&method(&fn_nodes, name)),
            data_live.contains(&method(&data_nodes, name)),
            "{name}'s liveness moved"
        );
    }
}
