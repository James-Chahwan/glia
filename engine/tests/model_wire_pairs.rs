//! DT.1g acceptance: `glia_graph::client_wires` over the committed
//! `models-http-shop` fixture (an Angular web client and a gin API, built
//! with `generate_many` over the dirs its key.json names). Every call site's
//! response / request shape resolves to its client models through the
//! persisted symbols, every route the `http` resolver paired it with lists
//! its handlers' known-2xx server sites, and each server value resolves
//! through the model the Go build wrote (DT.1f), flattened through the bases
//! its schema's `extends` names.
//!
//! Names alone would pair wrongly here: the TS `Article` is the Go
//! `CatalogEntry` (GET /api/articles writes `[]models.CatalogEntry`), and the
//! Go `models.Article` of the same name is never serialised.
//!
//! fired_on: `[models] client_wires hits=9 routed=9 ...`, printed below.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use glia_engine::generate_many;
use glia_graph::{
    ClientWire, ClientWires, MergedGraph, ResolvedShape, RouteHandler, WireDirection, client_wires,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
}

/// The repo paths a fixture's key.json `dirs` names, in order.
fn fixture_dirs(name: &str) -> Vec<String> {
    let root = fixture(name);
    let key: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("key.json")).unwrap()).unwrap();
    key["dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| root.join(d.as_str().unwrap()).to_str().unwrap().to_string())
        .collect()
}

fn shop() -> MergedGraph {
    generate_many(&fixture_dirs("models-http-shop"))
        .expect("models-http-shop builds")
        .merged
}

fn qname(m: &MergedGraph, id: Option<glia_core::NodeId>) -> String {
    let Some(id) = id else { return String::new() };
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

fn wire<'a>(cw: &'a ClientWires, route: &str) -> &'a ClientWire {
    let of: Vec<&ClientWire> = cw
        .wires
        .iter()
        .filter(|w| w.routes.iter().any(|r| r.route_qname == route))
        .collect();
    assert_eq!(of.len(), 1, "one wire routed to {route}: {:#?}", cw.wires);
    of[0]
}

fn only_handler<'a>(w: &'a ClientWire) -> &'a RouteHandler {
    assert_eq!(w.routes.len(), 1, "{w:#?}");
    assert_eq!(w.routes[0].rule, "exact", "{w:#?}");
    assert_eq!(w.routes[0].handlers.len(), 1, "{w:#?}");
    &w.routes[0].handlers[0]
}

/// The one server site of `w`'s handler in `dir`.
fn server<'a>(w: &'a ClientWire, dir: WireDirection) -> &'a ResolvedShape {
    let sites = only_handler(w).sites(dir);
    assert_eq!(sites.len(), 1, "{dir:?} sites of {w:#?}");
    &sites[0].shape
}

fn keys(s: &ResolvedShape) -> Vec<&str> {
    s.fields.keys().map(String::as_str).collect()
}

#[test]
fn client_wires_over_the_shop_fixture() {
    let m = shop();
    let cw = client_wires(&m);
    let marker = cw.marker();
    eprintln!("{marker}");
    assert_eq!(
        marker,
        "[models] client_wires hits=9 routed=9 response=9 request=2 client_models=5 unresolved_client=0 \
         server_sites=11 server_unknown_status=0 open=0 weak_pairing=0"
    );

    // Nine routed hits; eleven directions with a side on both ends.
    let mut both = 0;
    for w in &cw.wires {
        for dir in WireDirection::ALL {
            if w.client(dir).is_some() {
                assert_eq!(only_handler(w).sites(dir).len(), 1, "{dir:?} of {w:#?}");
                both += 1;
            }
        }
    }
    assert_eq!(both, 11);
    let routes: Vec<&str> = cw
        .wires
        .iter()
        .map(|w| w.routes[0].route_qname.as_str())
        .collect();
    assert_eq!(
        routes,
        [
            "GET /api/articles",
            "GET /api/invoice-pages/:n",
            "GET /api/invoices/:id",
            "GET /api/listings",
            "GET /api/listings/:id",
            "GET /api/summary/daily",
            "GET /api/summary/weekly",
            "PATCH /api/invoices/:id/status",
            "POST /api/invoices",
        ],
        "sorted by route qname"
    );

    // GET /api/invoices/:id: TS Invoice against the Go Invoice; the 404 gin.H
    // body is never a site (skipped by status).
    let get = wire(&cw, "GET /api/invoices/:id");
    assert_eq!(get.file, "src/app/services/invoice.service.ts");
    assert_eq!(get.line, 13);
    let client = get.client(WireDirection::Response).unwrap();
    assert_eq!(
        qname(&m, client.model),
        "src::app::models::invoice::Invoice"
    );
    assert_eq!(
        keys(client),
        [
            "couponCode",
            "customerName",
            "id",
            "issuedAt",
            "items",
            "status",
            "totalCents"
        ]
    );
    assert!(!client.fields["couponCode"].required);
    let items = &client.fields["items"].value;
    assert!(items.array);
    assert_eq!(
        qname(&m, items.model),
        "src::app::models::invoice::InvoiceItem",
        "bound in invoice.ts itself"
    );
    assert_eq!(keys(items), ["qty", "sku", "unitCents"]);
    assert!(
        client.fields["status"].value.model.is_none(),
        "InvoiceStatus is a type alias, no model"
    );
    assert_eq!(
        only_handler(get).qname,
        "handlers::handlers::Handlers::GetInvoice"
    );
    let site = &only_handler(get).response;
    assert_eq!(
        (site.len(), site[0].status, site[0].line),
        (1, Some(200), 39)
    );
    let srv = &site[0].shape;
    assert_eq!(qname(&m, srv.model), "models::invoice::Invoice");
    assert_eq!(
        keys(srv),
        [
            "customer_name",
            "id",
            "issuedAt",
            "items",
            "status",
            "totalCents"
        ],
        "json:\"-\" Note never travels"
    );
    assert_eq!(
        srv.fields["customer_name"].member.as_deref(),
        Some("CustomerName")
    );
    assert_eq!(
        qname(&m, srv.fields["items"].value.model),
        "models::invoice::InvoiceItem"
    );
    assert!(!srv.open && !client.open);

    // POST /api/invoices: NewInvoice against the bound CreateInvoiceRequest;
    // the 201 response; the 400 gin.H is no site.
    let post = wire(&cw, "POST /api/invoices");
    let req = post.client(WireDirection::Request).unwrap();
    assert_eq!(
        qname(&m, req.model),
        "src::app::models::invoice::NewInvoice"
    );
    let bind = server(post, WireDirection::Request);
    assert_eq!(
        qname(&m, bind.model),
        "models::invoice::CreateInvoiceRequest"
    );
    assert!(bind.fields["customerName"].bind_required);
    assert_eq!(only_handler(post).request[0].status, None);
    assert_eq!(only_handler(post).response[0].status, Some(201));
    assert_eq!(
        qname(&m, server(post, WireDirection::Response).model),
        "models::invoice::Invoice"
    );

    // PATCH .../status: the inline `{ status }` body against StatusUpdate.
    let patch = wire(&cw, "PATCH /api/invoices/:id/status");
    let body = patch.client(WireDirection::Request).unwrap();
    assert_eq!((body.model, keys(body)), (None, vec!["status"]));
    assert_eq!(
        qname(&m, server(patch, WireDirection::Request).model),
        "models::invoice::StatusUpdate"
    );

    // GET /api/invoice-pages/:n: both envelopes; `page` is an untyped
    // strconv.Atoi call on the server.
    let page = wire(&cw, "GET /api/invoice-pages/:n");
    let env = page.client(WireDirection::Response).unwrap();
    assert_eq!(keys(env), ["invoices", "page"]);
    assert_eq!(
        qname(&m, env.fields["invoices"].value.model),
        "src::app::models::invoice::Invoice"
    );
    let senv = server(page, WireDirection::Response);
    assert_eq!(keys(senv), ["invoices", "page"]);
    assert!(senv.fields["invoices"].value.array);
    assert_eq!(
        qname(&m, senv.fields["invoices"].value.model),
        "models::invoice::Invoice"
    );
    assert!(senv.fields["page"].value.open && !env.fields["page"].value.open);

    // GET /api/articles: the server writes CatalogEntry, never the stored
    // models.Article of the client's name.
    let art = wire(&cw, "GET /api/articles");
    let ca = art.client(WireDirection::Response).unwrap();
    assert!(ca.array);
    assert_eq!(qname(&m, ca.model), "src::app::models::catalog::Article");
    let sa = server(art, WireDirection::Response);
    assert!(sa.array);
    assert_eq!(qname(&m, sa.model), "models::catalog::CatalogEntry");
    let mut server_models = BTreeSet::new();
    for w in &cw.wires {
        for side in &w.routes {
            for h in &side.handlers {
                for s in h.response.iter().chain(&h.request) {
                    server_models.insert(qname(&m, s.shape.model));
                    for f in s.shape.fields.values() {
                        server_models.insert(qname(&m, f.value.model));
                    }
                }
            }
        }
    }
    assert!(
        !server_models.contains("models::catalog::Article"),
        "{server_models:?}"
    );

    // GET /api/listings: the envelope's `listings` is ListingWithSeller,
    // flattened with its embedded Listing's fields.
    let lst = wire(&cw, "GET /api/listings");
    let cl = lst.client(WireDirection::Response).unwrap();
    assert_eq!(
        qname(&m, cl.fields["listings"].value.model),
        "src::app::models::catalog::Listing"
    );
    let sl = &server(lst, WireDirection::Response).fields["listings"].value;
    assert!(sl.array && !sl.open);
    assert_eq!(qname(&m, sl.model), "models::catalog::ListingWithSeller");
    assert_eq!(keys(sl), ["id", "priceCents", "sellerName", "title"]);
    assert_eq!(
        qname(&m, sl.fields["id"].declared_in),
        "models::catalog::Listing"
    );
    assert_eq!(
        qname(&m, sl.fields["sellerName"].declared_in),
        "models::catalog::ListingWithSeller"
    );

    let one = wire(&cw, "GET /api/listings/:id");
    assert_eq!(
        qname(&m, server(one, WireDirection::Response).model),
        "models::catalog::Listing"
    );
    for (route, model) in [
        ("GET /api/summary/daily", "models::catalog::DailySummary"),
        ("GET /api/summary/weekly", "models::catalog::WeeklySummary"),
    ] {
        let w = wire(&cw, route);
        assert_eq!(
            qname(&m, w.client(WireDirection::Response).unwrap().model),
            "src::app::models::catalog::Summary"
        );
        assert_eq!(qname(&m, server(w, WireDirection::Response).model), model);
    }

    // Every hit's root client model resolves (an envelope's keys included).
    for w in &cw.wires {
        let r = w.client(WireDirection::Response).unwrap();
        let named = r.model.is_some() || r.fields.values().any(|f| f.value.model.is_some());
        assert!(named, "{w:#?}");
    }
}

#[test]
fn client_wires_are_deterministic() {
    let a = client_wires(&shop());
    let b = client_wires(&shop());
    assert_eq!(a, b);
}
