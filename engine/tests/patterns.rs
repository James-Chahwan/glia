//! LE.7a — pattern conformance (experimental): route handlers grouped by the
//! service they sit in, each handler's role chain down its calls to the first
//! effect sink as a signature, the population's most frequent signature as its
//! convention, and every other handler as a located DIVERGENCE (heuristic: a
//! convention is observed, never declared). Delta mode keeps the conventions
//! of the whole after graph and reports only the divergences a change touched.
//!
//! The Go shop: `handlers/handlers.go` (gin routes registered in `Register`),
//! `service/service.go` (functions calling the repository) and
//! `repository/repository.go` (raw SQL through `database/sql`). File stems
//! equal package names: Go cross-package calls resolve by file stem, so a
//! `repository/repo.go` would lose every call into it (the LA-core gap LE.7a
//! records). The engine prints `[patterns] experimental populations=..` per
//! answer; run with `--nocapture` to see it.

mod git_fixture;

use std::collections::BTreeMap;

use git_fixture::GitRepo;
use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use glia_core::{
    Cell, CellPayload, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId,
};
use glia_engine::delta::graph_delta_vs_rev;
use glia_engine::generate_one;
use glia_engine::patterns::{
    Divergence, PatternArgs, PatternReport, Population, pattern_conformance,
    pattern_conformance_delta,
};
use glia_graph::{MergedGraph, RepoGraph, SymbolTable};

const CONVENTION: &str = "handler>service>repository>db";
const DIRECT_SIGNATURE: &str = "handler>repository>db";

/// One route handler of the shop: its route, its function, and what it calls:
/// `service` names the service function it goes through (`None`: straight to
/// the repository), `repo` the repository function that runs `sql`.
struct H {
    verb: &'static str,
    path: &'static str,
    name: &'static str,
    service: Option<&'static str>,
    repo: &'static str,
    sql: &'static str,
}

const GET_USER: H = H {
    verb: "GET",
    path: "/users/:id",
    name: "GetUserHandler",
    service: Some("GetUser"),
    repo: "FindUser",
    sql: "SELECT name FROM users WHERE id = $1",
};
const CREATE_USER: H = H {
    verb: "POST",
    path: "/users",
    name: "CreateUserHandler",
    service: Some("CreateUser"),
    repo: "InsertUser",
    sql: "INSERT INTO users (name) VALUES ($1)",
};
const GET_ORDER: H = H {
    verb: "GET",
    path: "/orders/:id",
    name: "GetOrderHandler",
    service: Some("GetOrder"),
    repo: "FindOrder",
    sql: "SELECT item FROM orders WHERE id = $1",
};
const LIST_PRODUCTS: H = H {
    verb: "GET",
    path: "/products",
    name: "ListProductsHandler",
    service: Some("ListProducts"),
    repo: "AllProducts",
    sql: "SELECT name FROM products WHERE id > $1",
};
const CREATE_PAYMENT: H = H {
    verb: "POST",
    path: "/payments",
    name: "CreatePaymentHandler",
    service: Some("CreatePayment"),
    repo: "InsertPayment",
    sql: "INSERT INTO payments (amount) VALUES ($1)",
};
/// The one handler that skips the service layer.
const RAW_ORDER: H = H {
    verb: "POST",
    path: "/orders",
    name: "RawOrderHandler",
    service: None,
    repo: "SaveOrder",
    sql: "INSERT INTO orders (item) VALUES ($1)",
};
const RAW_REFUND: H = H {
    verb: "POST",
    path: "/refunds",
    name: "RawRefundHandler",
    service: None,
    repo: "SaveRefund",
    sql: "INSERT INTO refunds (item) VALUES ($1)",
};
const RAW_INVOICE: H = H {
    verb: "POST",
    path: "/invoices",
    name: "RawInvoiceHandler",
    service: None,
    repo: "SaveInvoice",
    sql: "INSERT INTO invoices (item) VALUES ($1)",
};

const FIVE: [&H; 5] = [
    &GET_USER,
    &CREATE_USER,
    &GET_ORDER,
    &LIST_PRODUCTS,
    &CREATE_PAYMENT,
];

fn six() -> Vec<&'static H> {
    let mut hs = FIVE.to_vec();
    hs.push(&RAW_ORDER);
    hs
}

fn handlers_go(hs: &[&H]) -> String {
    let direct = hs.iter().any(|h| h.service.is_none());
    let layered = hs.iter().any(|h| h.service.is_some());
    let mut s = String::from("package handlers\n\nimport (\n\t\"net/http\"\n\n");
    if direct {
        s.push_str("\t\"example.com/shop/repository\"\n");
    }
    if layered {
        s.push_str("\t\"example.com/shop/service\"\n");
    }
    s.push_str("\t\"github.com/gin-gonic/gin\"\n)\n\nfunc Register(r *gin.Engine) {\n");
    for h in hs {
        s.push_str(&format!("\tr.{}(\"{}\", {})\n", h.verb, h.path, h.name));
    }
    s.push_str("}\n");
    for h in hs {
        let callee = match h.service {
            Some(f) => format!("service.{f}"),
            None => format!("repository.{}", h.repo),
        };
        s.push_str(&format!(
            "\nfunc {}(c *gin.Context) {{\n\tv := {callee}(c.Param(\"id\"))\n\tc.JSON(http.StatusOK, v)\n}}\n",
            h.name
        ));
    }
    s
}

fn service_go(hs: &[&H], get_user_extra: bool) -> String {
    let mut s = String::from("package service\n\nimport \"example.com/shop/repository\"\n");
    for h in hs {
        let Some(f) = h.service else { continue };
        let extra = if get_user_extra && f == "GetUser" {
            "\tid = id + \"\"\n"
        } else {
            ""
        };
        s.push_str(&format!(
            "\nfunc {f}(id string) string {{\n{extra}\treturn repository.{}(id)\n}}\n",
            h.repo
        ));
    }
    s
}

/// `with_sql = false` leaves every function's body free of SQL.
fn repository_go(hs: &[&H], with_sql: bool) -> String {
    let mut s = String::from("package repository\n\nimport \"database/sql\"\n\nvar db *sql.DB\n");
    for h in hs {
        let body = if with_sql {
            format!("\tdb.Exec(\"{}\", id)\n", h.sql)
        } else {
            String::new()
        };
        s.push_str(&format!(
            "\nfunc {}(id string) string {{\n{body}\treturn id\n}}\n",
            h.repo
        ));
    }
    s
}

const GO_MOD: &str =
    "module example.com/shop\n\ngo 1.21\n\nrequire github.com/gin-gonic/gin v1.9.1\n";

/// An unrelated package no handler reaches.
const UTIL_GO: &str = "package util\n\nfunc Clamp(v int) int {\n\treturn v\n}\n";

/// Write the shop with handlers `hs` into `repo`'s work tree.
fn write_shop(repo: &GitRepo, hs: &[&H]) {
    repo.write("go.mod", GO_MOD);
    repo.write("handlers/handlers.go", &handlers_go(hs));
    repo.write("service/service.go", &service_go(hs, false));
    repo.write("repository/repository.go", &repository_go(hs, true));
}

/// The shop with handlers `hs`, uncommitted.
fn shop(hs: &[&H]) -> GitRepo {
    let repo = GitRepo::init();
    write_shop(&repo, hs);
    repo
}

fn build(repo: &GitRepo) -> (MergedGraph, BTreeMap<u64, String>) {
    let path = repo.root().to_str().expect("utf-8 temp path");
    let r = generate_one(path).expect("generate_one");
    (r.merged, r.repo_labels)
}

fn whole(repo: &GitRepo) -> PatternReport {
    let (merged, labels) = build(repo);
    pattern_conformance(&merged, &labels, &PatternArgs::default())
}

fn population<'a>(r: &'a PatternReport, service: &str) -> &'a Population {
    r.populations
        .iter()
        .find(|p| p.service == service)
        .unwrap_or_else(|| panic!("no population {service} in {r:#?}"))
}

fn handlers_of(ds: &[Divergence]) -> Vec<&str> {
    ds.iter().map(|d| d.handler.as_str()).collect()
}

/// 1-based line of the first line of `text` that contains `needle`.
fn line_of(text: &str, needle: &str) -> i64 {
    let at = text
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` not in the fixture"));
    i64::try_from(at).expect("small fixture") + 1
}

/// (1) Five handlers go handler > service > repository > db, one goes
/// straight to the repository: one population of six in `handlers`, the
/// convention held by five, and the direct handler the one located
/// DIVERGENCE. The service functions take their role from their name (the
/// `service` package), the repository functions from their data access.
#[test]
fn go_handler_convention_5_of_6() {
    let repo = shop(&six());
    let r = whole(&repo);
    assert!(r.experimental);
    assert!(!r.delta_mode);
    assert_eq!((r.handlers, r.judged, r.skipped_small), (6, 1, 0), "{r:#?}");
    assert_eq!(r.populations.len(), 1, "{r:#?}");

    let p = population(&r, "handlers");
    assert_eq!(p.role, "handler");
    assert_eq!(p.size, 6);
    assert_eq!(p.status, "judged");
    assert_eq!(p.convention.as_deref(), Some(CONVENTION));
    assert_eq!(p.matching, 5);
    assert_eq!(p.verdict.as_deref(), Some("5/6"));
    assert_eq!(
        p.signatures,
        vec![
            (CONVENTION.to_string(), 5),
            (DIRECT_SIGNATURE.to_string(), 1)
        ]
    );
    let sources = |role: &str| -> Vec<(&str, usize)> {
        p.role_sources
            .get(role)
            .map(|m| m.iter().map(|(s, n)| (*s, *n)).collect())
            .unwrap_or_default()
    };
    assert_eq!(sources("service"), [("name", 5)]);
    assert_eq!(sources("repository"), [("edge", 6)]);
    let totals: Vec<(&str, usize)> = r.role_sources.iter().map(|(s, n)| (*s, *n)).collect();
    assert_eq!(totals, [("edge", 6), ("kind", 0), ("name", 5)]);

    assert_eq!(
        handlers_of(&r.divergences),
        ["handlers::handlers::RawOrderHandler"]
    );
    assert_eq!(handlers_of(&p.exceptions), handlers_of(&r.divergences));
    let d = &r.divergences[0];
    assert_eq!((d.verdict, d.tier), ("DIVERGENCE", "heuristic"));
    assert_eq!(d.service, "handlers");
    assert_eq!(d.signature, DIRECT_SIGNATURE);
    assert_eq!(d.convention, CONVENTION);
    assert_eq!((d.matching, d.population), (5, 6));
    assert_eq!(d.route_method.as_deref(), Some("POST"));
    assert_eq!(d.route_path.as_deref(), Some("/orders"));

    let handlers_src = handlers_go(&six());
    assert_eq!(d.file.as_deref(), Some("handlers/handlers.go"));
    assert_eq!(
        d.line,
        Some(line_of(&handlers_src, "func RawOrderHandler("))
    );

    // The path: the handler's call, then the repository's data access, each
    // hop at the site that asserted it (1-based).
    let hops: Vec<(&str, &str, &str, bool)> = d
        .path
        .iter()
        .map(|h| {
            (
                h.from_qname.as_str(),
                h.to_qname.as_str(),
                h.category,
                h.backward,
            )
        })
        .collect();
    assert_eq!(
        hops,
        [
            (
                "handlers::handlers::RawOrderHandler",
                "repository::repository::SaveOrder",
                "CALLS",
                false
            ),
            (
                "repository::repository::SaveOrder",
                "data_entity:sql:orders",
                "ACCESSES_DATA",
                false
            ),
        ]
    );
    assert_eq!(d.path[0].site_file.as_deref(), Some("handlers/handlers.go"));
    let raw_body = handlers_src
        .split("func RawOrderHandler(")
        .nth(1)
        .expect("handler body");
    let call_line = line_of(&handlers_src, "func RawOrderHandler(")
        + line_of(raw_body, "repository.SaveOrder(")
        - 1;
    assert_eq!(d.path[0].site_line, Some(call_line));
    let repo_src = repository_go(&six(), true);
    assert_eq!(
        d.path[1].site_file.as_deref(),
        Some("repository/repository.go")
    );
    let save = repo_src
        .split("func SaveOrder(")
        .nth(1)
        .expect("SaveOrder body");
    let sql_line = line_of(&repo_src, "func SaveOrder(") + line_of(save, "db.Exec(") - 1;
    assert_eq!(d.path[1].site_line, Some(sql_line));

    let roles: Vec<(&str, &str, &str)> = d
        .role_sources
        .iter()
        .map(|s| (s.qname.as_str(), s.role, s.source))
        .collect();
    assert_eq!(
        roles,
        [
            ("handlers::handlers::RawOrderHandler", "handler", "edge"),
            ("repository::repository::SaveOrder", "repository", "edge"),
        ]
    );
}

/// (2) Four handlers are below the default support of five: the population
/// is listed with its signatures but gets no verdict.
#[test]
fn below_min_support_no_verdict() {
    let hs: Vec<&H> = vec![&GET_USER, &CREATE_USER, &GET_ORDER, &RAW_ORDER];
    let r = whole(&shop(&hs));
    assert_eq!((r.handlers, r.judged, r.skipped_small), (4, 0, 1), "{r:#?}");
    assert!(r.divergences.is_empty(), "{r:#?}");
    let p = population(&r, "handlers");
    assert_eq!(p.status, "too_small");
    assert_eq!(p.convention, None);
    assert_eq!(p.verdict, None);
    assert_eq!(p.matching, 0);
    assert!(p.exceptions.is_empty());
    assert_eq!(
        p.signatures,
        vec![
            (CONVENTION.to_string(), 3),
            (DIRECT_SIGNATURE.to_string(), 1)
        ]
    );
}

/// (3) Three layered handlers and three direct ones: no signature reaches
/// 75% of the six, so no convention is declared and nothing diverges.
#[test]
fn no_convention_when_split() {
    let hs: Vec<&H> = vec![
        &GET_USER,
        &CREATE_USER,
        &GET_ORDER,
        &RAW_ORDER,
        &RAW_REFUND,
        &RAW_INVOICE,
    ];
    let r = whole(&shop(&hs));
    assert_eq!((r.handlers, r.judged, r.skipped_small), (6, 0, 0), "{r:#?}");
    let p = population(&r, "handlers");
    assert_eq!(p.status, "no_convention");
    assert_eq!(p.convention, None);
    assert_eq!(p.verdict, None);
    assert!(p.exceptions.is_empty());
    assert!(r.divergences.is_empty(), "{r:#?}");
    assert_eq!(
        p.signatures,
        vec![
            (DIRECT_SIGNATURE.to_string(), 3),
            (CONVENTION.to_string(), 3)
        ]
    );
}

// ---------------------------------------------------------------------------
// (4) A hand-built graph: roles come from kind before name.
// ---------------------------------------------------------------------------

fn test_repo() -> RepoId {
    RepoId::from_canonical("test://patterns")
}

#[derive(Default)]
struct G {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    nav: CodeNav,
}

impl G {
    fn node(&mut self, kind: NodeKindId, qname: &str, at: Option<(&str, i64)>) -> NodeId {
        let id = NodeId::from_parts(GRAPH_TYPE, test_repo(), kind, qname);
        let name = qname.rsplit("::").next().unwrap_or(qname);
        self.nav.record(id, name, qname, kind, None);
        let cells = at
            .map(|(file, row)| {
                vec![Cell {
                    kind: cell_type::POSITION,
                    payload: CellPayload::Json(format!(
                        r#"{{"file":"{file}","start_line":{row}}}"#
                    )),
                }]
            })
            .unwrap_or_default();
        self.nodes.push(Node {
            id,
            repo: test_repo(),
            confidence: Confidence::Strong,
            cells,
        });
        id
    }

    fn edge(&mut self, from: NodeId, to: NodeId, category: EdgeCategoryId) {
        self.edges.push(Edge {
            from,
            to,
            category,
            confidence: Confidence::Strong,
            cells: vec![],
        });
    }

    fn merged(self) -> MergedGraph {
        MergedGraph::new(vec![RepoGraph {
            repo: test_repo(),
            nodes: self.nodes,
            edges: self.edges,
            nav: self.nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        }])
    }
}

/// `GET /foo` -> `GetFoo` -CALLS-> `FooHelper::run` (which a `helper_kind`
/// node `FooHelper` CONTAINS, the shape `services.rs` emits for a SERVICE)
/// -CALLS-> `Writer::save` -ACCESSES_DATA-> `data_entity:sql:foo`.
fn helper_graph(helper_kind: NodeKindId) -> MergedGraph {
    let mut g = G::default();
    let route = g.node(node_kind::ROUTE, "GET /foo", None);
    let handler = g.node(
        node_kind::FUNCTION,
        "api::handlers::GetFoo",
        Some(("api/handlers.go", 2)),
    );
    let helper = g.node(
        helper_kind,
        "core::helpers::FooHelper",
        Some(("core/helpers.go", 0)),
    );
    let run = g.node(
        node_kind::METHOD,
        "core::helpers::FooHelper::run",
        Some(("core/helpers.go", 3)),
    );
    let save = g.node(
        node_kind::METHOD,
        "core::db::Writer::save",
        Some(("core/db.go", 5)),
    );
    let table = g.node(node_kind::DATA_ENTITY, "data_entity:sql:foo", None);
    g.edge(route, handler, edge_category::HANDLED_BY);
    g.edge(helper, run, edge_category::CONTAINS);
    g.edge(handler, run, edge_category::CALLS);
    g.edge(run, save, edge_category::CALLS);
    g.edge(save, table, edge_category::ACCESSES_DATA);
    g.merged()
}

/// (4) A SERVICE-kind node named `FooHelper` makes the method it contains a
/// service (role source `kind`) although no name says so; the same graph
/// with `FooHelper` a plain CLASS drops it as a helper.
#[test]
fn kind_roles_beat_name() {
    let mut args = PatternArgs::default();
    args.min_support = 1;
    let labels = BTreeMap::new();

    let r = pattern_conformance(&helper_graph(node_kind::SERVICE), &labels, &args);
    let p = population(&r, "api");
    assert_eq!(p.status, "judged", "{r:#?}");
    assert_eq!(p.convention.as_deref(), Some(CONVENTION));
    assert_eq!(
        p.role_sources
            .get("service")
            .map(|m| m.iter().map(|(s, n)| (*s, *n)).collect::<Vec<_>>()),
        Some(vec![("kind", 1)])
    );
    assert_eq!(
        p.role_sources
            .get("repository")
            .map(|m| m.iter().map(|(s, n)| (*s, *n)).collect::<Vec<_>>()),
        Some(vec![("edge", 1)])
    );
    let totals: Vec<(&str, usize)> = r.role_sources.iter().map(|(s, n)| (*s, *n)).collect();
    assert_eq!(totals, [("edge", 1), ("kind", 1), ("name", 0)]);

    let r = pattern_conformance(&helper_graph(node_kind::CLASS), &labels, &args);
    let p = population(&r, "api");
    assert_eq!(p.convention.as_deref(), Some(DIRECT_SIGNATURE), "{r:#?}");
    assert!(!p.role_sources.contains_key("service"), "{r:#?}");
}

/// `seen` handlers calling `Writer::save` (which writes `data_entity:sql:foo`)
/// and `blind` handlers with no out-edge, all in `api/handlers.go`.
fn fan_graph(seen: usize, blind: usize) -> MergedGraph {
    let mut g = G::default();
    let save = g.node(
        node_kind::FUNCTION,
        "core::db::Writer::save",
        Some(("core/db.go", 1)),
    );
    let table = g.node(node_kind::DATA_ENTITY, "data_entity:sql:foo", None);
    g.edge(save, table, edge_category::ACCESSES_DATA);
    for i in 0..seen + blind {
        let route = g.node(node_kind::ROUTE, &format!("GET /r{i}"), None);
        let handler = g.node(
            node_kind::FUNCTION,
            &format!("api::handlers::H{i}"),
            Some(("api/handlers.go", i64::try_from(i).expect("small") * 4)),
        );
        g.edge(route, handler, edge_category::HANDLED_BY);
        if i < seen {
            g.edge(handler, save, edge_category::CALLS);
        }
    }
    g.merged()
}

/// (4b) `handler>(no effect)` is never the convention: five blind handlers and
/// one the graph follows is no convention, not "the followed one diverges".
/// Under a real convention a blind handler (a health check) is still listed.
#[test]
fn blind_chains_are_never_the_convention() {
    let labels = BTreeMap::new();
    let args = PatternArgs::default();

    let r = pattern_conformance(&fan_graph(1, 5), &labels, &args);
    let p = population(&r, "api");
    assert_eq!(p.status, "no_convention", "{r:#?}");
    assert_eq!(p.convention, None);
    assert!(r.divergences.is_empty(), "{r:#?}");
    assert_eq!(
        p.signatures,
        vec![
            ("handler>(no effect)".to_string(), 5),
            (DIRECT_SIGNATURE.to_string(), 1)
        ]
    );

    let r = pattern_conformance(&fan_graph(5, 1), &labels, &args);
    let p = population(&r, "api");
    assert_eq!(p.convention.as_deref(), Some(DIRECT_SIGNATURE), "{r:#?}");
    assert_eq!(p.verdict.as_deref(), Some("5/6"));
    assert_eq!(handlers_of(&r.divergences), ["api::handlers::H5"]);
    let d = &r.divergences[0];
    assert_eq!(d.signature, "handler>(no effect)");
    assert!(d.path.is_empty());
    assert_eq!(d.route_path.as_deref(), Some("/r5"));
    assert_eq!(
        (d.file.as_deref(), d.line),
        (Some("api/handlers.go"), Some(21))
    );
}

// ---------------------------------------------------------------------------
// (5) Delta mode.
// ---------------------------------------------------------------------------

fn delta_report(repo: &GitRepo) -> (PatternReport, PatternReport) {
    let d = graph_delta_vs_rev(repo.path(), "HEAD").expect("graph delta vs HEAD");
    let args = PatternArgs::default();
    let labels = &d.after.repo_labels;
    let delta = pattern_conformance_delta(&d.after.merged, labels, &d.delta, &args);
    let whole = pattern_conformance(&d.after.merged, labels, &args);
    (delta, whole)
}

/// (5) The five layered handlers committed; the direct one added in the
/// working tree is the delta's one divergence. Then, with all six committed,
/// an unrelated change (a removed package, an edit inside a conventional
/// handler's service) leaves the whole-graph divergence in place while delta
/// mode reports none.
#[test]
fn delta_mode_reports_only_touched() {
    let repo = shop(&FIVE);
    repo.commit("five layered handlers");
    write_shop(&repo, &six());
    let (delta, whole) = delta_report(&repo);
    assert!(delta.delta_mode);
    assert_eq!(
        handlers_of(&delta.divergences),
        ["handlers::handlers::RawOrderHandler"],
        "{delta:#?}"
    );
    assert_eq!(
        handlers_of(&whole.divergences),
        handlers_of(&delta.divergences)
    );
    // Conventions come from the whole after graph.
    let p = population(&delta, "handlers");
    assert_eq!((p.size, p.matching), (6, 5));

    let repo = shop(&six());
    repo.write("util/util.go", UTIL_GO);
    repo.commit("six handlers");
    repo.remove("util/util.go");
    repo.write("service/service.go", &service_go(&six(), true));
    let (delta, whole) = delta_report(&repo);
    assert_eq!(
        handlers_of(&whole.divergences),
        ["handlers::handlers::RawOrderHandler"],
        "{whole:#?}"
    );
    assert!(delta.divergences.is_empty(), "{delta:#?}");
    assert_eq!(
        handlers_of(&population(&delta, "handlers").exceptions),
        ["handlers::handlers::RawOrderHandler"]
    );
}

/// (5b) A moved handler is touched: `git mv` of the handler file moves all
/// six handlers, and the divergent one is reported under its new qname.
#[test]
fn delta_mode_moved_handler_is_touched() {
    let repo = shop(&six());
    repo.commit("six handlers");
    repo.git_mv("handlers/handlers.go", "handlers/routes.go");
    assert!(repo.root().join("handlers/routes.go").is_file());
    let (delta, _) = delta_report(&repo);
    assert_eq!(
        handlers_of(&delta.divergences),
        ["handlers::routes::RawOrderHandler"],
        "{delta:#?}"
    );
}

/// (5c) A change on a divergent handler's path is touched: the committed
/// repository runs no SQL, so the direct handler reaches no effect; the
/// working tree adds the SQL, and the direct handler (itself unchanged) now
/// diverges through the edited repository function and its added edge.
#[test]
fn delta_mode_path_edit_is_touched() {
    let repo = GitRepo::init();
    write_shop(&repo, &six());
    repo.write("repository/repository.go", &repository_go(&six(), false));
    repo.commit("no sql yet");
    repo.write("repository/repository.go", &repository_go(&six(), true));
    let (delta, whole) = delta_report(&repo);
    assert_eq!(
        handlers_of(&delta.divergences),
        ["handlers::handlers::RawOrderHandler"],
        "{delta:#?}"
    );
    assert_eq!(delta.divergences[0].signature, DIRECT_SIGNATURE);
    assert_eq!(
        handlers_of(&whole.divergences),
        handlers_of(&delta.divergences)
    );
}

/// (6) Two builds of one tree, and two answers over one build, serialise to
/// the same bytes.
#[test]
fn deterministic() {
    let repo = shop(&six());
    let (m1, l1) = build(&repo);
    let (m2, l2) = build(&repo);
    let args = PatternArgs::default();
    let a = serde_json::to_string(&pattern_conformance(&m1, &l1, &args)).expect("json");
    let b = serde_json::to_string(&pattern_conformance(&m1, &l1, &args)).expect("json");
    let c = serde_json::to_string(&pattern_conformance(&m2, &l2, &args)).expect("json");
    assert_eq!(a, b);
    assert_eq!(a, c);
    assert!(a.contains("\"experimental\":true"), "{a}");
}
