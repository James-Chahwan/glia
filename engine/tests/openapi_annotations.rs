//! LA.15a / LA.15b acceptance: OpenAPI annotations on a handler become contract ops on a
//! REAL build of the committed substrate-gap fixtures.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until the
//! end-of-wave rebuild; this grades the working tree directly. Each op is the
//! A10.1 contract DOC_SECTION, keyed on the ROUTE its handler implements, and
//! `link_contract_routes` pairs it with that ROUTE unchanged. Run with
//! `-- --nocapture` to see the `[openapi-annot]` / `[contract-link]` markers.

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, CellTypeId, NodeId, NodeKindId};
use repo_graph_engine::generate_one;
use repo_graph_graph::MergedGraph;

fn build(rel: &str) -> MergedGraph {
    let dir = format!(
        "{}/../bench/substrate-gap/fixtures/{rel}",
        env!("CARGO_MANIFEST_DIR")
    );
    generate_one(&dir).expect("fixture builds").merged
}

/// The id of the node of `kind` whose qname is exactly `qname`, if any.
fn find(m: &MergedGraph, kind: NodeKindId, qname: &str) -> Option<NodeId> {
    m.graphs.iter().find_map(|g| {
        g.nav
            .qname_by_id
            .iter()
            .find(|(id, q)| q.as_str() == qname && g.nav.kind_by_id.get(id) == Some(&kind))
            .map(|(id, _)| *id)
    })
}

fn node(m: &MergedGraph, kind: NodeKindId, qname: &str) -> NodeId {
    find(m, kind, qname).unwrap_or_else(|| panic!("no {qname} node of kind {kind:?}"))
}

/// The payload of `id`'s first cell of type `ct`.
fn cell(m: &MergedGraph, id: NodeId, ct: CellTypeId) -> String {
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter())
        .find_map(|c| match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) if c.kind == ct => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn documents(m: &MergedGraph, from: NodeId) -> Vec<NodeId> {
    m.all_edges()
        .filter(|e| e.from == from && e.category == edge_category::DOCUMENTS)
        .map(|e| e.to)
        .collect()
}

#[test]
fn annotation_ops_document_their_routes() {
    // springdoc: @Operation + two @ApiResponse on a composed Spring handler.
    let m = build("contract-annot-springdoc");
    let op = node(
        &m,
        node_kind::DOC_SECTION,
        "contract::UserController::GET:/api/users/{id}",
    );
    let route = node(&m, node_kind::ROUTE, "GET /api/users/{id}");
    let any = node(&m, node_kind::ROUTE, "ANY /api/users");
    assert_eq!(
        documents(&m, op),
        vec![route],
        "the op documents its handler's route only"
    );
    assert_ne!(route, any);
    let origin = cell(&m, op, cell_type::ORIGIN);
    for want in [
        r#""provenance":"contract","source":"springdoc","method":"GET","path":"/api/users/{id}""#,
        r#""operation_id":"getUser""#,
        r#""responses":["200","404"]"#,
    ] {
        assert!(origin.contains(want), "{want} not in {origin}");
    }
    assert!(
        cell(&m, op, cell_type::POSITION).contains(r#""start_line":9,"#),
        "0-indexed first annotation line"
    );
    assert!(
        find(
            &m,
            node_kind::DOC_SECTION,
            "contract::UserController::POST:/api/users"
        )
        .is_none(),
        "an un-annotated handler is not a declared op"
    );

    // Swashbuckle: [SwaggerOperation] + [ProducesResponseType] typeof / StatusCodes.
    let m = build("contract-annot-swashbuckle");
    let op = node(
        &m,
        node_kind::DOC_SECTION,
        "contract::UsersController::GET:/api/users/{id}",
    );
    let route = node(&m, node_kind::ROUTE, "GET /api/users/{id}");
    assert_eq!(documents(&m, op), vec![route]);
    let origin = cell(&m, op, cell_type::ORIGIN);
    for want in [
        r#""source":"swashbuckle""#,
        r#""operation_id":"GetUser""#,
        r#""responses":["200","404"]"#,
        r#""response_types":{"200":"UserDto"}"#,
    ] {
        assert!(origin.contains(want), "{want} not in {origin}");
    }
    assert!(
        find(
            &m,
            node_kind::DOC_SECTION,
            "contract::UsersController::DELETE:/api/users/{id}"
        )
        .is_none()
    );

    // NestJS: @Controller('users') + @Get(':id') compose the key.
    let m = build("contract-annot-nestjs");
    let op = node(
        &m,
        node_kind::DOC_SECTION,
        "contract::users.controller::GET:/users/:id",
    );
    let route = node(&m, node_kind::ROUTE, "route:/users/:id");
    assert_eq!(documents(&m, op), vec![route]);
    let origin = cell(&m, op, cell_type::ORIGIN);
    for want in [r#""source":"nestjs""#, r#""responses":["200"]"#] {
        assert!(origin.contains(want), "{want} not in {origin}");
    }
    assert!(
        find(
            &m,
            node_kind::DOC_SECTION,
            "contract::users.controller::POST:/users"
        )
        .is_none()
    );
}

/// LA.15b: swaggo and rswag DECLARE method + path themselves, so the op keys
/// on no route and no owner; `link_contract_routes` pairs it with the gin /
/// Rails ROUTE by placeholder folding (`{id}` against `:id`).
#[test]
fn path_declaring_annotations_document_routes() {
    // swaggo: a `// @Router /users/{id} [get]` godoc block above a gin handler.
    let m = build("contract-annot-swaggo");
    let op = node(
        &m,
        node_kind::DOC_SECTION,
        "contract::users::GET:/users/{id}",
    );
    // LB.11a: the gin route is one node per (method, path).
    let route = node(&m, node_kind::ROUTE, "GET /users/:id");
    assert_eq!(documents(&m, op), vec![route]);
    let origin = cell(&m, op, cell_type::ORIGIN);
    for want in [
        r#""provenance":"contract","source":"swaggo","method":"GET","path":"/users/{id}""#,
        r#""operation_id":"getUser""#,
        r#""responses":["200","404"]"#,
    ] {
        assert!(origin.contains(want), "{want} not in {origin}");
    }
    assert!(
        cell(&m, op, cell_type::POSITION).contains(r#""start_line":10,"#),
        "0-indexed @Router line"
    );
    assert!(
        find(&m, node_kind::DOC_SECTION, "contract::users::GET:/health").is_none(),
        "an ordinary comment is not a swaggo block"
    );

    // rswag: `path '/users/{id}' do` + `get '...' do` in a request spec under
    // spec/. The ORIGIN is set at extraction, so the test-dir provenance pass
    // leaves it `contract`.
    let m = build("contract-annot-rswag");
    let op = node(
        &m,
        node_kind::DOC_SECTION,
        "contract::users_spec::GET:/users/{id}",
    );
    let route = node(&m, node_kind::ROUTE, "GET /users/:id");
    assert_eq!(documents(&m, op), vec![route]);
    let origin = cell(&m, op, cell_type::ORIGIN);
    for want in [
        r#""provenance":"contract","source":"rswag""#,
        r#""operation_id":"getUser""#,
        r#""responses":["200","404"]"#,
    ] {
        assert!(origin.contains(want), "{want} not in {origin}");
    }
    assert!(
        find(&m, node_kind::ROUTE, "GET Retrieves a user").is_none()
            && find(&m, node_kind::ROUTE, "route:Retrieves a user").is_none(),
        "the spec's verb block is not a route"
    );
}
