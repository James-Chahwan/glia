//! A8.3 — `scope: Option<&str>` on the P3 answer-shaped primitives.
//!
//! Proves the two ORDERING claims, not merely that a filter exists:
//!   - `blast_radius_by_qname` scopes BEFORE `truncate(top_k)`, so a scoped
//!     `--top-k` spends its budget in scope instead of on whatever PPR liked
//!     globally (the fixture is built so the globally top-ranked node is OUT of
//!     scope — filter-after-truncate would return nothing);
//!   - `resolve_signal_located` scopes the SEEDS before `activate`, so the PPR
//!     scores themselves change. `resolve_frame` matches POSITION files by
//!     BASENAME ONLY, so an unscoped frame naming `utils.py` seeds every
//!     same-named file in a monorepo and skews the ranking of the real one.
//! Plus the `/`-boundary rule and the keep-unlocatable policy.

use std::path::Path;

use repo_graph_engine::{
    GenerateResult, blast_radius_by_qname, generate_one, governing_docs, locate_node,
    node_in_scope, resolve_signal_located,
};

/// A three-subproject monorepo. `services/api` calls both into `shared` and
/// within itself; `shared/util.py::call_shared` has three callers so it
/// outranks the in-scope `local_dep` globally. A flask route and a `requests`
/// call give us a ROUTE node (no POSITION, no ENDPOINT_HIT — genuinely
/// unlocatable) and an ENDPOINT node (locatable only via its ENDPOINT_HIT
/// `file`). Two markdown docs, one at the root and one under `docs/`, both name
/// `handle` so `governing_docs` has something to narrow.
fn write_fixture(dir: &Path) {
    for d in ["services/api", "shared", "web", "docs"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    std::fs::write(
        dir.join("services/api/handler.py"),
        "from shared.util import call_shared\nfrom services.api.deps import local_dep\n\n\n\
         def handle():\n    return call_shared() + local_dep()\n",
    )
    .unwrap();
    std::fs::write(dir.join("services/api/deps.py"), "def local_dep():\n    return 3\n").unwrap();
    std::fs::write(
        dir.join("services/api/app.py"),
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
         @app.route(\"/v1/items\")\ndef items():\n    return []\n",
    )
    .unwrap();
    std::fs::write(dir.join("shared/util.py"), "def call_shared():\n    return 1\n").unwrap();
    std::fs::write(
        dir.join("shared/more.py"),
        "from shared.util import call_shared\n\n\ndef more():\n    return call_shared()\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("shared/again.py"),
        "from shared.util import call_shared\n\n\ndef again():\n    return call_shared()\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("web/client.py"),
        "import requests\n\n\ndef web_entry():\n    return requests.get(\"http://api/v1/items\")\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("README.md"),
        "# Readme\n\nThe `handle` entrypoint is described here.\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("docs/rules.md"),
        "# Rules\n\nRules governing `handle` live here.\n",
    )
    .unwrap();
}

fn fixture() -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().unwrap();
    write_fixture(td.path());
    let result = generate_one(td.path().to_str().unwrap()).expect("generate_one");
    (td, result)
}

const SCOPE: &str = "services/api";

#[test]
fn blast_radius_scope_filters_and_runs_before_truncate() {
    let (_td, result) = fixture();
    let m = &result.merged;

    // (a) unscoped reaches across into shared/; scoped to services/api it does not.
    let all = blast_radius_by_qname(m, "handle", "both", 4, None, false, None).unwrap();
    assert!(
        all.iter().any(|a| a.file.as_deref() == Some("shared/util.py")),
        "unscoped radius should reach shared/util.py; got {:?}",
        all.iter().map(|a| (&a.qname, &a.file)).collect::<Vec<_>>()
    );
    let scoped = blast_radius_by_qname(m, "handle", "both", 4, None, false, Some(SCOPE)).unwrap();
    assert!(
        !scoped.is_empty(),
        "scoped radius should keep the in-scope nodes"
    );
    assert!(
        scoped
            .iter()
            .all(|a| a.file.as_deref().is_some_and(|f| f.starts_with("services/api/"))),
        "scoped radius must contain only services/api nodes; got {:?}",
        scoped.iter().map(|a| (&a.qname, &a.file)).collect::<Vec<_>>()
    );

    // (b) ORDER: scope must run BEFORE truncate(top_k). Precondition — the
    // globally top-ranked node is OUT of scope (call_shared has three callers,
    // local_dep one), so truncate-then-filter would return an EMPTY answer.
    assert!(
        !all[0].file.as_deref().is_some_and(|f| f.starts_with("services/api/")),
        "precondition: the globally top-ranked node should be out of scope; got {:?}",
        all[0].file
    );
    let one = blast_radius_by_qname(m, "handle", "both", 4, Some(1), false, Some(SCOPE)).unwrap();
    assert_eq!(
        one.len(),
        1,
        "top_k=1 under scope must still return one IN-SCOPE node — an empty \
         answer here means the filter ran after truncate"
    );
    assert_eq!(one[0].file.as_deref(), Some("services/api/deps.py"));

    // (c) BOUNDARY: a prefix that stops mid-segment must not match.
    let boundary =
        blast_radius_by_qname(m, "handle", "both", 4, None, false, Some("services/ap")).unwrap();
    assert!(
        boundary
            .iter()
            .all(|a| !a.file.as_deref().is_some_and(|f| f.starts_with("services/api/"))),
        "`services/ap` must not prefix-match `services/api/...`; got {:?}",
        boundary.iter().map(|a| &a.file).collect::<Vec<_>>()
    );
}

#[test]
fn resolve_scope_filters_seeds_before_ppr() {
    let (_td, result) = fixture();
    let m = &result.merged;
    let trace = "Traceback (most recent call last):\n  \
        File \"services/api/handler.py\", line 6, in handle\n    \
        return call_shared() + local_dep()\n  \
        File \"web/client.py\", line 5, in web_entry\n    \
        return requests.get(\"http://api/v1/items\")\n";

    let unscoped = resolve_signal_located(m, trace, "stacktrace", None, None);
    assert_eq!(
        unscoped.len(),
        2,
        "both frames should resolve unscoped; got {:?}",
        unscoped.iter().map(|n| (&n.qname, &n.file)).collect::<Vec<_>>()
    );

    // (d) scoped: only the handler frame survives, AND its PPR score changes,
    // because the seed set handed to `activate` shrank — proof the filter ran
    // pre-PPR rather than on the rendered result.
    let scoped = resolve_signal_located(m, trace, "stacktrace", None, Some(SCOPE));
    assert_eq!(scoped.len(), 1, "only the in-scope frame should survive");
    assert_eq!(scoped[0].file.as_deref(), Some("services/api/handler.py"));
    let before = unscoped
        .iter()
        .find(|n| n.id == scoped[0].id)
        .expect("the scoped node must also appear unscoped")
        .score;
    assert!(
        (before - scoped[0].score).abs() > 1e-9,
        "score must change when the seed set shrinks (pre-PPR filter): {before} vs {}",
        scoped[0].score
    );
}

#[test]
fn governing_docs_scope_drops_out_of_tree_sections() {
    let (_td, result) = fixture();
    let m = &result.merged;
    let all = governing_docs(m, "handle", None).unwrap();
    for want in ["README.md", "docs/rules.md"] {
        assert!(
            all.iter().any(|d| d.file.as_deref() == Some(want)),
            "unscoped governing_docs should include {want}; got {:?}",
            all.iter().map(|d| &d.file).collect::<Vec<_>>()
        );
    }
    // (e) scoped to docs/ drops the README-derived DOC_SECTION.
    let scoped = governing_docs(m, "handle", Some("docs")).unwrap();
    assert!(
        scoped.iter().all(|d| d.file.as_deref() != Some("README.md")),
        "scope=docs must drop the README section; got {:?}",
        scoped.iter().map(|d| &d.file).collect::<Vec<_>>()
    );
    assert!(
        scoped.iter().any(|d| d.file.as_deref() == Some("docs/rules.md")),
        "scope=docs must keep docs/rules.md"
    );
}

#[test]
fn unlocatable_nodes_are_kept_and_endpoint_hit_is_the_fallback() {
    let (_td, result) = fixture();
    let m = &result.merged;
    let by_kind = |want: &str| -> Vec<_> {
        m.graphs
            .iter()
            .flat_map(|g| g.nodes.iter())
            .map(|n| n.id)
            .filter(|id| locate_node(m, *id).2 == want)
            .collect::<Vec<_>>()
    };

    // (f) A ROUTE carries only ROUTE_METHOD (the bare HTTP verb) — no POSITION,
    // no ENDPOINT_HIT — so it is unlocatable and must be KEPT under any scope.
    // Dropping unlocatables would silently delete every ROUTE/ENDPOINT/DOC_SPACE
    // from a scoped answer, which is the cross-service half of the result.
    let routes = by_kind("ROUTE");
    assert!(!routes.is_empty(), "fixture should emit a flask ROUTE node");
    for r in &routes {
        assert!(locate_node(m, *r).3.is_none(), "ROUTE should have no POSITION");
        assert!(
            node_in_scope(m, *r, Some("no/such/dir")),
            "unlocatable nodes must be kept under any scope"
        );
    }

    // An ENDPOINT has no POSITION either, but its ENDPOINT_HIT cell names the
    // call site — the fallback makes it genuinely scopable rather than kept.
    let endpoints = by_kind("ENDPOINT");
    assert!(!endpoints.is_empty(), "fixture should emit an ENDPOINT node");
    for e in &endpoints {
        assert!(locate_node(m, *e).3.is_none(), "ENDPOINT should have no POSITION");
        assert!(
            node_in_scope(m, *e, Some("web")),
            "ENDPOINT_HIT file web/client.py should place it under `web`"
        );
        assert!(
            !node_in_scope(m, *e, Some(SCOPE)),
            "ENDPOINT_HIT file web/client.py is not under services/api"
        );
    }

    // scope=None is a strict no-op for everything.
    assert!(node_in_scope(m, m.graphs[0].nodes[0].id, None));
}
