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
//!
//! A8.6 extends it: a scope may also be a PROJECT label (`@shop/web`), which
//! `resolve_scope` turns into that project's path before any filter runs, so
//! a label answer must EQUAL the path answer — never approximate it.

use std::path::Path;

use repo_graph_engine::{
    GenerateResult, blast_radius_by_qname, generate_one, governing_docs, locate_node,
    node_in_scope, project_roots, resolve_scope, resolve_signal_located,
};

/// A three-subproject monorepo. `services/api` calls both into `shared` and
/// within itself; `shared/util.py::call_shared` has three callers so it
/// outranks the in-scope `local_dep` globally. A flask route and a `requests`
/// call give us a ROUTE node (no POSITION, located via the handler it is
/// HANDLED_BY — A3.6) and an ENDPOINT node (located via its ENDPOINT_HIT
/// `file`). A Django `path(...)` registration names no handler, so its ROUTE
/// is genuinely unlocatable. Two markdown docs, one at the root and one under
/// `docs/`, both name `handle` so `governing_docs` has something to narrow.
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
    std::fs::write(
        dir.join("services/api/urls.py"),
        "from django.urls import path\n\nurlpatterns = [path(\"/v1/legacy\", legacy_view)]\n",
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

    let unscoped = resolve_signal_located(m, trace, "stacktrace", None, None).results;
    assert_eq!(
        unscoped.len(),
        2,
        "both frames should resolve unscoped; got {:?}",
        unscoped.iter().map(|n| (&n.qname, &n.file)).collect::<Vec<_>>()
    );

    // (d) scoped: only the handler frame survives, AND its PPR score changes,
    // because the seed set handed to `activate` shrank — proof the filter ran
    // pre-PPR rather than on the rendered result.
    let scoped = resolve_signal_located(m, trace, "stacktrace", None, Some(SCOPE)).results;
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
    let all = governing_docs(m, "handle", None).results;
    for want in ["README.md", "docs/rules.md"] {
        assert!(
            all.iter().any(|d| d.file.as_deref() == Some(want)),
            "unscoped governing_docs should include {want}; got {:?}",
            all.iter().map(|d| &d.file).collect::<Vec<_>>()
        );
    }
    // (e) scoped to docs/ drops the README-derived DOC_SECTION.
    let scoped = governing_docs(m, "handle", Some("docs")).results;
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
fn unlocatable_nodes_are_kept_and_http_nodes_are_scoped_where_defined() {
    let (_td, result) = fixture();
    let m = &result.merged;
    let by_qname = |q: &str| {
        m.node_id_by_qname(q)
            .unwrap_or_else(|| panic!("fixture should emit `{q}`"))
    };

    // (f) KEEP-UNLOCATABLE. The Django route's ROUTE_METHOD is the bare verb
    // and it names no handler, so no `locate_node` tier places it: it must be
    // KEPT under any scope. Dropping unlocatables would silently delete them
    // from a scoped answer, which is the cross-service half of the result.
    let orphan = by_qname("ANY /v1/legacy");
    assert_eq!(locate_node(m, orphan).file, None, "a handler-less route has no span");
    assert!(
        node_in_scope(m, orphan, Some("no/such/dir")),
        "unlocatable nodes must be kept under any scope"
    );

    // A3.6: the flask ROUTE has no POSITION and only the bare verb, but it is
    // HANDLED_BY `items`, so it is located at — and scoped by — its handler's
    // file rather than kept as unlocatable.
    let route = by_qname("GET /v1/items");
    assert_eq!(
        locate_node(m, route).file.as_deref(),
        Some("services/api/app.py"),
        "the ROUTE borrows its handler's POSITION"
    );
    assert!(node_in_scope(m, route, Some(SCOPE)));
    assert!(
        !node_in_scope(m, route, Some("web")),
        "a located ROUTE is scoped like any other node"
    );

    // An ENDPOINT has no POSITION either; its ENDPOINT_HIT cell names the
    // call site, which is what `locate_node` and the scope filter both report.
    let endpoints: Vec<_> = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .map(|n| n.id)
        .filter(|id| locate_node(m, *id).kind == "ENDPOINT")
        .collect();
    assert!(!endpoints.is_empty(), "fixture should emit an ENDPOINT node");
    for e in &endpoints {
        assert_eq!(
            locate_node(m, *e).file.as_deref(),
            Some("web/client.py"),
            "ENDPOINT is located at its ENDPOINT_HIT call site"
        );
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

// ============================================================================
// A8.6 — a project LABEL is scope vocabulary too.
// ============================================================================

/// The committed A8.5 fixture: four manifest roots under ONE RepoId.
fn walk_project_roots() -> String {
    format!(
        "{}/../bench/substrate-gap/fixtures/walk-project-roots",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst);
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// The same four manifests plus a call graph that crosses a project boundary.
/// On the committed fixture every radius is EMPTY, so "label == path" there
/// would hold vacuously. Here `webEntry` calls `helper` inside `apps/web`, and
/// root-level `tools/run.ts` (under no nested root) calls `webEntry`.
fn project_fixture() -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    copy_tree(Path::new(&walk_project_roots()), d);
    std::fs::write(
        d.join("apps/web/index.ts"),
        "import { helper } from \"./helper\";\n\n\
         export function webEntry(): number {\n  return helper();\n}\n",
    )
    .unwrap();
    std::fs::write(
        d.join("apps/web/helper.ts"),
        "export function helper(): number {\n  return 1;\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(d.join("tools")).unwrap();
    std::fs::write(
        d.join("tools/run.ts"),
        "import { webEntry } from \"../apps/web/index\";\n\n\
         export function runAll(): number {\n  return webEntry();\n}\n",
    )
    .unwrap();
    let result = generate_one(d.to_str().unwrap()).expect("generate_one");
    (td, result)
}

fn qnames<T>(v: &[T], q: impl Fn(&T) -> &str) -> Vec<String> {
    let mut out: Vec<String> = v.iter().map(|x| q(x).to_string()).collect();
    out.sort();
    out
}

#[test]
fn project_roots_decodes_every_anchor_sorted_by_path() {
    let r = generate_one(&walk_project_roots()).expect("fixture builds");
    let roots = project_roots(&r.merged);
    let got: Vec<(&str, &str, &str, &str, &str)> = roots
        .iter()
        .map(|p| {
            (
                p.path.as_str(),
                p.label.as_str(),
                p.ecosystem.as_str(),
                p.manifest.as_str(),
                p.qname.as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (".", "shop-monorepo", "npm", "package.json", "project:."),
            ("apps/web", "@shop/web", "npm", "apps/web/package.json", "project:apps/web"),
            ("libs/core", "shop-core", "cargo", "libs/core/Cargo.toml", "project:libs/core"),
            (
                "services/api",
                "github.com/shop/api",
                "go",
                "services/api/go.mod",
                "project:services/api"
            ),
        ]
    );

    // Everything is read back out of the graph, so a graph reopened from a
    // `.gmap` (pyo3 `load_from_gmap`) answers identically — no side channel.
    let td = tempfile::tempdir().unwrap();
    repo_graph_store::write_merged_sharded(&r.merged, td.path()).expect("write .gmap");
    let loaded = repo_graph_store::read_merged_sharded(td.path()).expect("read .gmap");
    let reread = project_roots(&loaded);
    assert_eq!(
        serde_json::to_string(&reread).unwrap(),
        serde_json::to_string(&roots).unwrap(),
        "a reopened .gmap must list the same projects"
    );
}

#[test]
fn resolve_scope_maps_labels_and_passes_everything_else_through() {
    let r = generate_one(&walk_project_roots()).expect("fixture builds");
    let m = &r.merged;
    // label -> path, including a label that itself contains `/`.
    assert_eq!(resolve_scope(m, "@shop/web"), "apps/web");
    assert_eq!(resolve_scope(m, "github.com/shop/api"), "services/api");
    assert_eq!(resolve_scope(m, "shop-core"), "libs/core");
    // The root project resolves to `.`, which scopes to the whole repo.
    assert_eq!(resolve_scope(m, "shop-monorepo"), ".");
    // The full PROJECT qname is accepted too.
    assert_eq!(resolve_scope(m, "project:apps/web"), "apps/web");
    // IDEMPOTENT on a path: resolving a resolved scope changes nothing.
    assert_eq!(resolve_scope(m, "apps/web"), "apps/web");
    assert_eq!(resolve_scope(m, &resolve_scope(m, "@shop/web")), "apps/web");
    // Unknown strings — plain dirs included — pass through untouched, no error.
    assert_eq!(resolve_scope(m, "nonsense"), "nonsense");
    assert_eq!(resolve_scope(m, "apps"), "apps");
    // Exact match only: no case folding, no prefix guessing.
    assert_eq!(resolve_scope(m, "@SHOP/WEB"), "@SHOP/WEB");
    assert_eq!(resolve_scope(m, "@shop"), "@shop");
}

#[test]
fn a_label_scope_equals_its_path_scope() {
    let (_td, r) = project_fixture();
    let m = &r.merged;

    let all = blast_radius_by_qname(m, "webEntry", "both", 4, None, false, None).unwrap();
    let all_q = qnames(&all, |a| &a.qname);
    assert!(
        all_q.iter().any(|q| q.contains("runAll")) && all_q.iter().any(|q| q.contains("helper")),
        "precondition: unscoped radius reaches both tools/ and apps/web; got {all_q:?}"
    );

    let by_path =
        blast_radius_by_qname(m, "webEntry", "both", 4, None, false, Some("apps/web")).unwrap();
    let by_label =
        blast_radius_by_qname(m, "webEntry", "both", 4, None, false, Some("@shop/web")).unwrap();
    let path_q = qnames(&by_path, |a| &a.qname);
    assert!(!path_q.is_empty(), "the in-scope helper must survive");
    assert!(
        !path_q.iter().any(|q| q.contains("runAll")),
        "tools/run.ts is outside apps/web; got {path_q:?}"
    );
    assert_eq!(qnames(&by_label, |a| &a.qname), path_q, "label scope == path scope");
    // Same set AND same ranking — label resolution runs before the filter.
    let scores = |v: &[repo_graph_engine::BlastAnswer]| {
        v.iter().map(|a| (a.qname.clone(), a.score.to_bits())).collect::<Vec<_>>()
    };
    assert_eq!(scores(&by_label), scores(&by_path));

    // The ROOT project's label resolves to `.`, i.e. the whole repo.
    let root = blast_radius_by_qname(m, "webEntry", "both", 4, None, false, Some("shop-monorepo"))
        .unwrap();
    assert_eq!(qnames(&root, |a| &a.qname), all_q, "`.` scopes to everything");

    // A second primitive, through the same applier: `resolve` filters its
    // SEEDS, so equal seeds must give equal PPR scores too.
    let diff = "apps/web/helper.ts\ntools/run.ts\n";
    let located = |scope: &str| {
        resolve_signal_located(m, diff, "diff", None, Some(scope))
            .results
            .iter()
            .map(|n| (n.qname.clone(), n.score.to_bits()))
            .collect::<Vec<_>>()
    };
    let (res_path, res_label) = (located("apps/web"), located("@shop/web"));
    assert!(
        !res_path.is_empty() && res_path.iter().all(|(q, _)| q.starts_with("apps::web::")),
        "only apps/web seeds survive; got {res_path:?}"
    );
    assert_eq!(res_label, res_path, "label scope == path scope, scores included");
}
