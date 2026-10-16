//! DB.2a — `glia_engine::rules::CodeFacts`, the code domain's `FactSource`
//! over a `MergedGraph`: node / edge / cell rows from the graph, the lazy
//! attribute columns (qname, name, file, line, repo, project, parent, role,
//! origin) through one Locator, the `in_scope` / `under` builtins with
//! check's strict scope membership, and edge tiers through `why`.
//!
//! The probe fixture fx1: three manifest projects (web/, shared/,
//! services/api/); web/app.py imports shared.util.helper and
//! services.api.public.facade.get, `page()` calls helper and `other()` calls
//! get; shared/util.py `helper()` calls services.api.internal.db.query;
//! facade `get()` calls db.write; plus one overlay `[[edge]]`
//! web::app::page -> services::api::internal::db::write CALLS.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_activation::algo::GraphSource;
use glia_code_domain::cell_type;
use glia_core::{Cell, CellPayload};
use glia_engine::rules::{CODE_ATTRIBUTES, CodeFacts};
use glia_engine::why::why_edge;
use glia_engine::{Locator, generate_one};
use glia_graph::MergedGraph;
use glia_rules::eval::{Options, run};
use glia_rules::source::{FactSource, catalog_conflicts};
use glia_rules::value::{Tier, Value, ValueRef};

const WEB_APP: &str = "from shared.util import helper\n\
from services.api.public.facade import get\n\
\n\
\n\
def page():\n    return helper()\n\
\n\
\n\
def other():\n    return get()\n";
const SHARED_UTIL: &str =
    "from services.api.internal.db import query\n\n\ndef helper():\n    return query()\n";
const FACADE: &str =
    "from services.api.internal.db import write\n\n\ndef get():\n    return write()\n";
const DB: &str = "def query():\n    return 1\n\n\ndef write():\n    return 2\n";
const OVERLAY: &str = "version = 1\n\n[[edge]]\nfrom = \"web::app::page\"\nto = \"services::api::internal::db::write\"\ncategory = \"CALLS\"\n";

const FILES: [(&str, &str); 8] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("shared/pyproject.toml", "[project]\nname = \"shared\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
    ("web/app.py", WEB_APP),
    ("shared/util.py", SHARED_UTIL),
    ("services/api/public/facade.py", FACADE),
    ("services/api/internal/db.py", DB),
    (".glia/overlay.toml", OVERLAY),
];

/// fx1 built once per test; the tempdir is returned so it outlives the graph.
fn fx1() -> (tempfile::TempDir, MergedGraph) {
    build(&FILES)
}

/// One repo of `files`, built.
fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (rel, src) in files {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    (tmp, r.merged)
}

/// The rendered (qname) set of `ids`.
fn qnames<'a>(
    facts: &CodeFacts<'_>,
    ids: impl IntoIterator<Item = &'a glia_core::NodeId>,
) -> BTreeSet<String> {
    ids.into_iter().map(|id| facts.render_node(*id)).collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// Every row of output `pred` of `program`, each value rendered.
fn eval(facts: &CodeFacts<'_>, program: &str, pred: &str) -> Vec<Vec<String>> {
    let ev =
        run("<rules_facts>", program, facts, &Options::default()).expect("the program evaluates");
    ev.rows(pred)
        .expect("an output")
        .iter()
        .map(|r| r.values.iter().map(|v| ev.render(v)).collect())
        .collect()
}

/// The `(node, value)` rows of attribute `name`.
fn attr(facts: &CodeFacts<'_>, name: &str) -> Vec<(glia_core::NodeId, Value)> {
    let i = CODE_ATTRIBUTES
        .iter()
        .position(|a| a.name == name)
        .expect("a code attribute");
    let mut out = Vec::new();
    facts.attr_rows(i, &mut out);
    out
}

fn api_six() -> BTreeSet<String> {
    set(&[
        "project:services/api",
        "services::api::internal::db",
        "services::api::internal::db::query",
        "services::api::internal::db::write",
        "services::api::public::facade",
        "services::api::public::facade::get",
    ])
}

#[test]
fn in_scope_matches_check_membership() {
    let (_tmp, merged) = fx1();
    let facts = CodeFacts::new(&merged);
    assert!(
        catalog_conflicts(&facts).is_empty(),
        "{:?}",
        catalog_conflicts(&facts)
    );

    assert_eq!(
        qnames(&facts, facts.members("services/api").iter()),
        api_six()
    );
    // Literal, as check: no file sits under api/ and no PROJECT is at it.
    assert!(
        facts.members("api").is_empty(),
        "members is literal: a label is not resolved"
    );
    assert!(facts.members("services/ap").is_empty(), "segment boundary");

    // The builtin resolves the `api` label (services/api/pyproject.toml's name).
    let by_label: BTreeSet<String> = eval(&facts, "x(a) :- in_scope(a, \"api\").", "x")
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    let by_path: BTreeSet<String> = eval(&facts, "x(a) :- in_scope(a, \"services/api\").", "x")
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    assert_eq!(by_path, api_six());
    assert_eq!(by_label, by_path);
    // A project qname is a scope too, as every `--scope` flag accepts.
    let by_qname: BTreeSet<String> = eval(
        &facts,
        "x(a) :- in_scope(a, \"project:services/api\").",
        "x",
    )
    .into_iter()
    .map(|r| r[0].clone())
    .collect();
    assert_eq!(by_qname, api_six());
    // A scope no node sits in yields nothing.
    assert!(eval(&facts, "x(a) :- in_scope(a, \"nowhere\").", "x").is_empty());

    let project: BTreeMap<String, Vec<String>> =
        attr(&facts, "project")
            .into_iter()
            .fold(BTreeMap::new(), |mut m, (id, v)| {
                let Value::Str(s) = v else {
                    panic!("project is a string column")
                };
                m.entry(facts.render_node(id)).or_default().push(s);
                m
            });
    assert_eq!(project["services::api::internal::db::query"], ["api"]);
    assert_eq!(
        project["project:services/api"],
        ["api"],
        "a PROJECT is its own label"
    );
    assert_eq!(project["web::app::page"], ["web"]);
    assert_eq!(project["shared::util"], ["shared"]);

    let repo = attr(&facts, "repo");
    assert_eq!(
        repo.len(),
        facts.node_ids().len(),
        "every node has a repo row"
    );
    for (id, v) in &repo {
        assert_eq!(
            v,
            &Value::Str("repo#0".to_string()),
            "{}",
            facts.render_node(*id)
        );
    }
}

#[test]
fn attributes_come_from_nav_and_locator() {
    let (_tmp, merged) = fx1();
    let loc = Locator::new(&merged);
    // A shared Locator answers as the facts' own does.
    let facts = CodeFacts::with_locator(&merged, &loc);
    assert_eq!(attr(&facts, "file"), attr(&CodeFacts::new(&merged), "file"));
    let file: HashMap<_, _> = attr(&facts, "file").into_iter().collect();
    let line: HashMap<_, _> = attr(&facts, "line").into_iter().collect();
    let qname: HashMap<_, _> = attr(&facts, "qname").into_iter().collect();
    let name: HashMap<_, _> = attr(&facts, "name").into_iter().collect();
    let mut unlocated = 0usize;
    for id in facts.node_ids() {
        let at = loc.locate(id);
        assert_eq!(
            file.get(&id),
            at.file.clone().map(Value::Str).as_ref(),
            "{}",
            at.qname
        );
        assert_eq!(
            line.get(&id),
            at.line.map(Value::Int).as_ref(),
            "{}",
            at.qname
        );
        if at.file.is_none() {
            unlocated += 1;
        }
        let nav = merged
            .graphs
            .iter()
            .find_map(|g| Some((g.nav.qname_by_id.get(&id)?, g.nav.name_by_id.get(&id))))
            .expect("every node is named by a nav");
        assert_eq!(qname.get(&id), Some(&Value::Str(nav.0.clone())));
        assert_eq!(name.get(&id), nav.1.cloned().map(Value::Str).as_ref());
    }
    assert_eq!(file.len() + unlocated, facts.node_ids().len());
    // The 1-based line: page() is on line 5 of web/app.py.
    let page = facts
        .node_ids()
        .into_iter()
        .find(|id| facts.render_node(*id) == "web::app::page");
    let page = page.expect("web::app::page");
    assert_eq!(file[&page], Value::Str("web/app.py".to_string()));
    assert_eq!(line[&page], Value::Int(5));

    let parents = eval(&facts, "p(c, q) :- parent(c, q).", "p");
    assert!(
        parents.contains(&vec!["web::app::page".to_string(), "web::app".to_string()]),
        "{parents:?}"
    );
    // A FUNCTION carries no role.
    assert!(
        eval(&facts, "r(n, v) :- role(n, v).", "r")
            .iter()
            .all(|r| r[0] != "web::app::page")
    );
}

#[test]
fn edge_tier_is_why() {
    let (_tmp, merged) = fx1();
    let facts = CodeFacts::new(&merged);
    // Per (from, to, category): the multiset of tiers the facts give and why gives,
    // and of (tier, note) as the located rows carry them.
    type Key = (String, String, &'static str);
    let mut ours: BTreeMap<Key, Vec<&'static str>> = BTreeMap::new();
    let mut rows: BTreeMap<Key, Vec<(&'static str, Option<String>)>> = BTreeMap::new();
    let mut keys: BTreeMap<Key, (glia_core::NodeId, glia_core::NodeId)> = BTreeMap::new();
    for (i, e) in facts.edges().enumerate() {
        let cat = glia_code_domain::edge_category::name(e.category);
        let key = (facts.render_node(e.from), facts.render_node(e.to), cat);
        ours.entry(key.clone())
            .or_default()
            .push(facts.edge_tier(i, e).name());
        let row = facts.locate_edge(e);
        assert_eq!(
            (row.from_qname.as_str(), row.to_qname.as_str(), row.category),
            (key.0.as_str(), key.1.as_str(), cat)
        );
        rows.entry(key.clone())
            .or_default()
            .push((row.tier, row.note));
        keys.insert(key, (e.from, e.to));
    }
    assert!(ours.len() > 10, "{}", ours.len());
    for (key, tiers) in &mut ours {
        let (from, to) = keys[key];
        let w = why_edge(&merged, &key.0, &key.1, Some(key.2)).expect("why answers");
        let why_rows: Vec<_> = w
            .edges
            .iter()
            .filter(|r| r.from_id == from.0 && r.to_id == to.0)
            .collect();
        let mut theirs: Vec<&'static str> = why_rows.iter().map(|r| r.tier).collect();
        tiers.sort_unstable();
        theirs.sort_unstable();
        assert_eq!(tiers, &theirs, "{key:?}");
        let mut noted: Vec<_> = why_rows.iter().map(|r| (r.tier, r.note.clone())).collect();
        let located = rows.get_mut(key).expect("rows");
        noted.sort();
        located.sort();
        assert_eq!(located, &noted, "{key:?}");
    }
    // The overlay edge is located at its stanza, the parsed call at its call site.
    let overlay = facts
        .edges()
        .find(|e| {
            facts.render_node(e.from) == "web::app::page"
                && facts.render_node(e.to) == "services::api::internal::db::write"
        })
        .expect("the overlay edge");
    let row = facts.locate_edge(overlay);
    assert_eq!(
        (row.file.as_deref(), row.line, row.emitter.as_deref()),
        (Some(".glia/overlay.toml"), Some(3), Some("overlay:edge"))
    );
    let call = facts
        .edges()
        .find(|e| {
            facts.render_node(e.from) == "web::app::page"
                && facts.render_node(e.to) == "shared::util::helper"
                && glia_code_domain::edge_category::name(e.category) == "CALLS"
        })
        .expect("page -> helper");
    let row = facts.locate_edge(call);
    assert_eq!(
        (row.file.as_deref(), row.line, row.tier),
        (Some("web/app.py"), Some(6), "fact")
    );
    let calls = |from: &str, to: &str| ours[&(from.to_string(), to.to_string(), "CALLS")].clone();
    assert_eq!(
        calls("web::app::page", "services::api::internal::db::write"),
        ["heuristic"]
    );
    assert_eq!(calls("web::app::page", "shared::util::helper"), ["fact"]);
    assert_eq!(
        calls("shared::util::helper", "services::api::internal::db::query"),
        ["fact"]
    );
    assert_eq!(
        calls(
            "services::api::public::facade::get",
            "services::api::internal::db::write"
        ),
        ["fact"]
    );
}

#[test]
fn under_is_segment_bounded() {
    let (_tmp, merged) = fx1();
    let facts = CodeFacts::new(&merged);
    let builtins = facts.builtins();
    let under = builtins
        .iter()
        .find(|b| b.decl().name == "under")
        .expect("the under builtin");
    let holds = |path: &str, prefix: &str| {
        let mut out = Vec::new();
        under
            .call(
                &[Some(ValueRef::Str(path)), Some(ValueRef::Str(prefix))],
                &mut out,
            )
            .expect("a bound call");
        match out.as_slice() {
            [] => false,
            [row] => {
                assert_eq!(row.tier, Tier::Fact);
                true
            }
            _ => panic!("a bound call yields one row at most"),
        }
    };
    for (path, prefix, want) in [
        ("web/app.py", "web", true),
        ("webhooks/a.py", "web", false),
        ("x/y.py", ".", true),
        ("./web/", "web", true),
        ("web", "web", true),
        ("web/app.py", "", true),
        ("web/app.py", "web/app.py", true),
        ("web/app.py", "/web/", true),
        ("services/api/x.py", "services/ap", false),
        ("shared/util.py", "web", false),
    ] {
        assert_eq!(holds(path, prefix), want, "under({path:?}, {prefix:?})");
    }
    let rows = eval(
        &facts,
        "f(\"web/app.py\").\nf(\"webhooks/a.py\").\nf(\"x/web/b.py\").\nok(p) :- f(p), under(p, \"web\").",
        "ok",
    );
    assert_eq!(rows, vec![vec!["web/app.py".to_string()]]);
    // The scope builtin is declared [Node, Str] with bind [May, Must].
    let in_scope = builtins
        .iter()
        .find(|b| b.decl().name == "in_scope")
        .expect("in_scope");
    let d = in_scope.decl();
    assert_eq!(
        d.cols,
        vec![glia_rules::ir::ColType::Node, glia_rules::ir::ColType::Str]
    );
    assert_eq!(
        d.bind,
        vec![glia_rules::ir::BindReq::May, glia_rules::ir::BindReq::Must]
    );
    assert_eq!(d.kind, glia_rules::ir::BuiltinKind::Domain);
}

#[test]
fn lazy_columns() {
    let (_tmp, merged) = fx1();
    let facts = CodeFacts::new(&merged);
    let rows = eval(
        &facts,
        "x(a, b) :- edge(a, b, \"CALLS\"), in_scope(a, \"web\"), in_scope(b, \"shared\").",
        "x",
    );
    assert_eq!(
        rows,
        vec![vec![
            "web::app::page".to_string(),
            "shared::util::helper".to_string()
        ]]
    );
    let loaded = facts.loaded_attributes();
    assert!(
        !loaded.contains(&"file") && !loaded.contains(&"line"),
        "{loaded:?}"
    );
    assert!(loaded.is_empty(), "no attribute was named: {loaded:?}");
    // A program that names a column loads it, once, in load order.
    eval(&facts, "q(n, v) :- qname(n, v).", "q");
    eval(&facts, "q(n, v) :- qname(n, v), line(n, l), l > 0.", "q");
    assert_eq!(facts.loaded_attributes(), ["qname", "line"]);
}

#[test]
fn one_node_per_id() {
    let (_tmp, merged) = fx1();
    let facts = CodeFacts::new(&merged);
    let loc = Locator::new(&merged);
    let ids = facts.node_ids();
    let unique: HashSet<_> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "node_ids has no duplicate");
    // First-occurrence order of the merged graph's own node order.
    let mut seen = HashSet::new();
    let first: Vec<_> = merged
        .node_ids()
        .into_iter()
        .filter(|id| seen.insert(*id))
        .collect();
    assert_eq!(ids, first);
    for id in &ids {
        assert_eq!(facts.render_node(*id), loc.locate(*id).qname);
        assert!(facts.node_kind(*id).is_some(), "{}", facts.render_node(*id));
    }
    assert_eq!(qnames(&facts, ids.iter()).len(), 13, "fx1 has 13 nodes");

    // The same graph twice (every id in two language graphs): still one node per
    // id, in the same order, with the same rows.
    let mut doubled = merged.clone();
    doubled.graphs.extend(merged.graphs.iter().cloned());
    let twice = CodeFacts::new(&doubled);
    assert_eq!(twice.node_ids(), ids);
    assert_eq!(twice.members("services/api").len(), 6);
    let node_rows = |f: &CodeFacts<'_>| eval(f, "n(x, k) :- node(x, k).", "n");
    assert_eq!(node_rows(&twice), node_rows(&facts));
    assert_eq!(node_rows(&facts).len(), 13);
}

#[test]
fn role_and_origin_read_the_cells() {
    let (_tmp, merged) = fx1();
    // fx1's code nodes carry no ROLE and no ORIGIN provenance: give two of them some.
    let mut m = merged.clone();
    let named = |m: &MergedGraph, q: &str| {
        m.graphs.iter().enumerate().find_map(|(gi, g)| {
            let ni = g
                .nodes
                .iter()
                .position(|n| g.nav.qname_by_id.get(&n.id).map(String::as_str) == Some(q))?;
            Some((gi, ni))
        })
    };
    let (gi, ni) = named(&m, "services::api::public::facade::get").expect("get");
    m.graphs[gi].nodes[ni].cells.push(Cell {
        kind: cell_type::ORIGIN,
        payload: CellPayload::Json("{\"provenance\":\"external\"}".to_string()),
    });
    let (gi, ni) = named(&m, "web::app::page").expect("page");
    m.graphs[gi].nodes[ni].cells.push(Cell {
        kind: cell_type::ROLE,
        payload: CellPayload::Json("{\"roles\":[\"COMPONENT\"]}".to_string()),
    });
    let facts = CodeFacts::new(&m);
    // A PROJECT anchor's ORIGIN cell says `project_root`.
    let rows = |pairs: &[(&str, &str)]| -> Vec<Vec<String>> {
        pairs
            .iter()
            .map(|(a, b)| vec![(*a).to_string(), (*b).to_string()])
            .collect()
    };
    assert_eq!(
        eval(&facts, "o(n, v) :- origin(n, v).", "o"),
        rows(&[
            ("project:services/api", "project_root"),
            ("project:shared", "project_root"),
            ("project:web", "project_root"),
            ("services::api::public::facade::get", "external"),
        ])
    );
    assert_eq!(
        eval(&facts, "r(n, v) :- role(n, v).", "r"),
        vec![vec!["web::app::page".to_string(), "COMPONENT".to_string()]]
    );
    assert_eq!(facts.loaded_attributes(), ["origin", "role"]);
}

#[test]
fn repo_and_project_follow_merge_order() {
    let (_a, first) = fx1();
    // A second checkout whose api project carries another label.
    let mut files = FILES;
    files[2] = (
        "services/api/pyproject.toml",
        "[project]\nname = \"api-b\"\n",
    );
    let (_b, second) = build(&files);
    let mut graphs = first.graphs.clone();
    graphs.extend(second.graphs.iter().cloned());
    let both = MergedGraph {
        graphs,
        cross_edges: Vec::new(),
        pass_undo: Vec::new(),
    };
    let facts = CodeFacts::new(&both);
    let in_second: HashSet<_> = second.node_ids().into_iter().collect();
    let in_first: HashSet<_> = first.node_ids().into_iter().collect();
    assert!(
        in_first.is_disjoint(&in_second),
        "two checkouts mint distinct node ids"
    );
    for (id, v) in attr(&facts, "repo") {
        let want = if in_second.contains(&id) {
            "repo#1"
        } else {
            "repo#0"
        };
        assert_eq!(v, Value::Str(want.to_string()), "{}", facts.render_node(id));
    }
    // A node's project is one of its own repo's PROJECTs, never the other's.
    for (id, v) in attr(&facts, "project") {
        if facts.render_node(id).starts_with("services::api") {
            let want = if in_second.contains(&id) {
                "api-b"
            } else {
                "api"
            };
            assert_eq!(v, Value::Str(want.to_string()), "{}", facts.render_node(id));
        }
    }
    // Scope membership is not per repo: both checkouts' api code sits under the path.
    assert_eq!(facts.members("services/api").len(), 12);
}
