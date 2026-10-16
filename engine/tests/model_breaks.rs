//! DT.1o acceptance: inferred-model breaks against a git rev,
//! `glia_engine::contract_breaks::model_breaks` over the committed
//! `model-breaks-shop` fixture (engine/tests/data/model-breaks-shop: an
//! Angular order client, a gin API and an Apollo admin console over one SDL
//! `Product`; [`model_shop`]).
//!
//! (a) v2 renames Order.CustomerName's JSON tag (`customerName` ->
//! `customer_name`): one breaking `http_model` response row Order <-> Order
//! over its three exchanges, the rename located on both sides, advisory under
//! the default `fact` gate and blocking under `derived`. Against HEAD with
//! the work tree edited: (b) a new omitempty field is a compatible
//! `response_field_added`; (c) a `json:"-"` tag removes a field the client
//! reads; (d) a new `binding:"required"` field breaks the POST body; (e) a
//! client field the server never sends is `json_field_missing`; (f) an SDL
//! field rename removes `stock` from both selections; (g) nothing changed:
//! the absence counts the seven judged exchanges.
//!
//! fired_on, once per case: `[model-breaks] base=<rev> min_tier=<t>
//! exchanges=7 http=5 graphql=2 rows=<R> ... new_pairings=0 lost=0
//! unjudged=0`: `cargo test -p glia-engine --test model_breaks --
//! --nocapture 2>&1 | grep '^\[model-breaks\] base='`.

mod git_fixture;
mod model_shop;

use git_fixture::GitRepo;
use glia_engine::contract_breaks::{
    MODEL_MIN_TIERS, ModelBreak, ModelBreakArgs, ModelBreaks, ModelChange, ModelExchange,
    ModelSite, model_breaks,
};
use glia_engine::contract_fields::FieldSide;
use glia_engine::delta::graph_delta_vs_rev;

fn breaks(repo: &GitRepo, base: &str, min_tier: &'static str) -> ModelBreaks {
    let rev = graph_delta_vs_rev(repo.path(), base).expect("the rev delta builds");
    let mut args = ModelBreakArgs::default();
    args.min_tier = min_tier;
    model_breaks(&rev, &args)
}

fn at(s: &ModelSite) -> String {
    format!(
        "{}:{}{}",
        s.file.as_deref().unwrap_or("?"),
        s.line.map_or_else(|| "?".to_string(), |l| l.to_string()),
        if s.at_base { "@base" } else { "" }
    )
}

fn opt_at(s: &Option<ModelSite>) -> String {
    s.as_ref().map_or_else(|| "-".to_string(), at)
}

fn side(f: &FieldSide) -> String {
    format!(
        "{} {} {}:{}",
        f.qname,
        f.format,
        f.file.as_deref().unwrap_or("?"),
        f.line.map_or_else(|| "?".to_string(), |l| l.to_string())
    )
}

fn exchange(e: &ModelExchange) -> String {
    format!(
        "{} {} {} / {}",
        e.key,
        e.tier,
        at(&e.client_site),
        at(&e.server_site)
    )
}

fn change(c: &ModelChange) -> String {
    format!(
        "{} {} {} {} breaking={} {} -> {} client={} server={}",
        c.field,
        c.change,
        c.side,
        c.rule,
        c.breaking,
        c.before.as_deref().unwrap_or("-"),
        c.after.as_deref().unwrap_or("-"),
        opt_at(&c.client),
        opt_at(&c.server)
    )
}

/// A row's head: pairing, direction, both sides, status, change, changed,
/// tier, blocking, note.
fn head(r: &ModelBreak) -> String {
    format!(
        "{} {} | {} | {} | {} {} {} {} blocking={} note={}",
        r.pairing,
        r.direction,
        side(&r.client),
        side(&r.server),
        r.status,
        r.change,
        r.changed,
        r.tier,
        r.blocking,
        r.note.unwrap_or("-")
    )
}

fn rows(mb: &ModelBreaks) -> Vec<(String, Vec<String>, Vec<String>)> {
    mb.rows
        .iter()
        .map(|r| {
            (
                head(r),
                r.exchanges.iter().map(exchange).collect(),
                r.changes.iter().map(change).collect(),
            )
        })
        .collect()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

const ORDER_TS: &str =
    "web::src::app::core::models::order::Order ts web/src/app/core/models/order.ts:9";
const ORDER_GO: &str = "api::internal::models::order::Order go api/internal/models/order.go:20";

/// The three response exchanges that join the TS Order with the Go Order.
fn order_exchanges() -> Vec<String> {
    strings(&[
        "GET /api/orders/:id derived web/src/app/core/services/order.service.ts:14 / api/internal/handlers/orders.go:33",
        "PATCH /api/orders/:id/status derived web/src/app/core/services/order.service.ts:22 / api/internal/handlers/orders.go:59",
        "POST /api/orders derived web/src/app/core/services/order.service.ts:18 / api/internal/handlers/orders.go:44",
    ])
}

fn marker(mb: &ModelBreaks, base: &str, tier: &str, rows: &str) -> String {
    let m = mb.marker().to_string();
    assert_eq!(
        m,
        format!(
            "[model-breaks] base={base} min_tier={tier} exchanges=7 http=5 graphql=2 {rows} new_pairings=0 lost=0 unjudged=0"
        )
    );
    m
}

#[test]
fn a_renamed_json_tag_breaks_the_client_that_reads_the_old_name() {
    let repo = model_shop::renamed();
    let mb = breaks(&repo, "HEAD~1", MODEL_MIN_TIERS[0]);
    assert_eq!(
        rows(&mb),
        [(
            format!(
                "http_model response | {ORDER_TS} | {ORDER_GO} | breaking modified server derived blocking=false note=-"
            ),
            order_exchanges(),
            strings(&[
                "customerName renamed server json_name_changed breaking=true customerName -> customer_name client=web/src/app/core/models/order.ts:11 server=api/internal/models/order.go:22"
            ]),
        )],
        "discountCode and sku are drifts before the change, the request exchanges and the GraphQL \
         selections did not move: {:#?}",
        mb.rows
    );
    assert_eq!(
        (
            mb.min_tier,
            mb.exchanges,
            mb.judged,
            mb.breaking,
            mb.blocking,
            mb.advisory,
            mb.compatible
        ),
        ("fact", 7, 7, 1, 0, 1, 0)
    );
    assert!(mb.absence.is_none());
    marker(
        &mb,
        "HEAD~1",
        "fact",
        "rows=1 breaking=1 blocking=0 advisory=1 compatible=0",
    );
}

#[test]
fn the_derived_gate_makes_the_rename_block() {
    let repo = model_shop::renamed();
    let mb = breaks(&repo, "HEAD~1", MODEL_MIN_TIERS[1]);
    assert_eq!(mb.rows.len(), 1, "{:#?}", mb.rows);
    let r = &mb.rows[0];
    assert_eq!(
        (r.status, r.tier, r.blocking),
        ("breaking", "derived", true)
    );
    assert_eq!((mb.blocking, mb.advisory), (1, 0));
    marker(
        &mb,
        "HEAD~1",
        "derived",
        "rows=1 breaking=1 blocking=1 advisory=0 compatible=0",
    );
}

#[test]
fn an_unknown_gate_spelling_is_judged_as_fact() {
    let repo = model_shop::renamed();
    let mb = breaks(&repo, "HEAD~1", "heuristic");
    assert_eq!(mb.min_tier, "fact");
    assert_eq!((mb.blocking, mb.advisory), (0, 1));
}

#[test]
fn b_a_new_server_field_is_compatible() {
    let repo = model_shop::renamed();
    model_shop::edit(
        &repo,
        "api/internal/models/order.go",
        "\tInternalNote string      `json:\"-\"`\n",
        "\tInternalNote string      `json:\"-\"`\n\tNotes        string      `json:\"notes,omitempty\"`\n",
    );
    let mb = breaks(&repo, "HEAD", MODEL_MIN_TIERS[0]);
    assert_eq!(
        rows(&mb),
        [(
            format!(
                "http_model response | {ORDER_TS} | {ORDER_GO} | compatible modified server derived blocking=false note=-"
            ),
            order_exchanges(),
            strings(&[
                "notes added server response_field_added breaking=false - -> string client=- server=api/internal/models/order.go:28"
            ]),
        )],
        "{:#?}",
        mb.rows
    );
    marker(
        &mb,
        "HEAD",
        "fact",
        "rows=1 breaking=0 blocking=0 advisory=0 compatible=1",
    );
}

#[test]
fn c_a_dash_tag_removes_a_field_the_client_reads() {
    let repo = model_shop::renamed();
    model_shop::edit(
        &repo,
        "api/internal/models/order.go",
        "`json:\"totalCents\"`",
        "`json:\"-\"`",
    );
    let mb = breaks(&repo, "HEAD", MODEL_MIN_TIERS[0]);
    assert_eq!(
        rows(&mb),
        [(
            format!(
                "http_model response | {ORDER_TS} | {ORDER_GO} | breaking modified server derived blocking=false note=-"
            ),
            order_exchanges(),
            strings(&[
                "totalCents removed server response_field_removed breaking=true int64 -> - client=web/src/app/core/models/order.ts:14 server=api/internal/models/order.go:25"
            ]),
        )],
        "the working tree's Order still declares TotalCents (its `json:\"-\"` line): {:#?}",
        mb.rows
    );
    marker(
        &mb,
        "HEAD",
        "fact",
        "rows=1 breaking=1 blocking=0 advisory=1 compatible=0",
    );
}

#[test]
fn d_a_new_required_bind_breaks_the_request() {
    let repo = model_shop::renamed();
    model_shop::edit(
        &repo,
        "api/internal/models/order.go",
        "\tLines        []OrderLine `json:\"lines\" binding:\"required\"`\n",
        "\tLines        []OrderLine `json:\"lines\" binding:\"required\"`\n\tEmail        string      `json:\"email\" binding:\"required\"`\n",
    );
    let mb = breaks(&repo, "HEAD", MODEL_MIN_TIERS[0]);
    assert_eq!(
        rows(&mb),
        [(
            "http_model request | web::src::app::core::models::order::NewOrder ts web/src/app/core/models/order.ts:19 | api::internal::models::order::PlaceOrderRequest go api/internal/models/order.go:30 | breaking modified server derived blocking=false note=-".to_string(),
            strings(&[
                "POST /api/orders derived web/src/app/core/services/order.service.ts:18 / api/internal/handlers/orders.go:38"
            ]),
            strings(&[
                "email added server new_required_request_field breaking=true - -> string client=- server=api/internal/models/order.go:33"
            ]),
        )],
        "{:#?}",
        mb.rows
    );
    marker(
        &mb,
        "HEAD",
        "fact",
        "rows=1 breaking=1 blocking=0 advisory=1 compatible=0",
    );
}

#[test]
fn e_a_client_field_the_server_never_sends_is_missing() {
    let repo = model_shop::renamed();
    model_shop::edit(
        &repo,
        "web/src/app/core/models/order.ts",
        "  createdAt: string;\n",
        "  createdAt: string;\n  trackingUrl: string;\n",
    );
    let mb = breaks(&repo, "HEAD", MODEL_MIN_TIERS[0]);
    assert_eq!(
        rows(&mb),
        [(
            format!(
                "http_model response | {ORDER_TS} | {ORDER_GO} | breaking modified client derived blocking=false note=-"
            ),
            order_exchanges(),
            strings(&[
                "trackingUrl added client json_field_missing breaking=true - -> string client=web/src/app/core/models/order.ts:17 server=-"
            ]),
        )],
        "{:#?}",
        mb.rows
    );
    marker(
        &mb,
        "HEAD",
        "fact",
        "rows=1 breaking=1 blocking=0 advisory=1 compatible=0",
    );
}

#[test]
fn f_a_schema_field_rename_removes_what_both_selections_read() {
    let repo = model_shop::renamed();
    model_shop::edit(
        &repo,
        "api/graph/schema.graphqls",
        "  stock: Int!",
        "  quantity: Int!",
    );
    let mb = breaks(&repo, "HEAD", MODEL_MIN_TIERS[0]);
    let product = "message:graphql:Product @api graphql api/graph/schema.graphqls:1";
    assert_eq!(
        rows(&mb),
        [
            (
                format!(
                    "graphql_selection selection | graphql_op:Restock @admin graphql admin/src/queries.ts:15 | {product} | breaking modified server derived blocking=false note=-"
                ),
                strings(&[
                    "mutation Restock restock derived admin/src/queries.ts:15 / api/graph/schema.graphqls:13"
                ]),
                strings(&[
                    "stock removed server schema_field_removed breaking=true Int! -> - client=admin/src/queries.ts:19 server=api/graph/schema.graphqls:1"
                ]),
            ),
            (
                format!(
                    "graphql_selection selection | graphql_op:StockList @admin graphql admin/src/queries.ts:3 | {product} | breaking modified server derived blocking=false note=-"
                ),
                strings(&[
                    "query StockList products derived admin/src/queries.ts:3 / api/graph/schema.graphqls:9"
                ]),
                strings(&[
                    "stock removed server schema_field_removed breaking=true Int! -> - client=admin/src/queries.ts:9 server=api/graph/schema.graphqls:1"
                ]),
            ),
        ],
        "quantity is selected by no one, sku was undefined before the change: {:#?}",
        mb.rows
    );
    marker(
        &mb,
        "HEAD",
        "fact",
        "rows=2 breaking=2 blocking=0 advisory=2 compatible=0",
    );
}

#[test]
fn g_no_change_is_an_absence_that_counts_the_judged_exchanges() {
    let repo = model_shop::renamed();
    let mb = breaks(&repo, "HEAD", MODEL_MIN_TIERS[0]);
    assert!(mb.rows.is_empty(), "{:#?}", mb.rows);
    let a = mb.absence.as_ref().expect("an absence");
    assert_eq!(
        (a.reason, a.tier, a.note.as_str()),
        (
            "no_match",
            "FACT",
            "7 client <-> server model exchanges judged vs HEAD, none changed what a client reads or sends"
        )
    );
    assert_eq!(a.mechanisms, ["HTTP_CALLS", "GRAPHQL_CALLS"]);
    marker(
        &mb,
        "HEAD",
        "fact",
        "rows=0 breaking=0 blocking=0 advisory=0 compatible=0",
    );
}

#[test]
fn model_breaks_are_deterministic() {
    let repo = model_shop::renamed();
    let render = |mb: &ModelBreaks| serde_json::to_string(mb).expect("serialises");
    let first = render(&breaks(&repo, "HEAD~1", MODEL_MIN_TIERS[0]));
    let second = render(&breaks(&repo, "HEAD~1", MODEL_MIN_TIERS[0]));
    assert_eq!(first, second);
}

#[test]
fn a_moved_call_site_pairs_through_its_route_not_as_a_new_pairing() {
    let repo = model_shop::renamed();
    repo.git_mv(
        "web/src/app/core/services/order.service.ts",
        "web/src/app/core/services/orders.service.ts",
    );
    model_shop::edit(
        &repo,
        "web/src/app/core/models/order.ts",
        "  createdAt: string;\n",
        "  createdAt: string;\n  trackingUrl: string;\n",
    );
    let mb = breaks(&repo, "HEAD", MODEL_MIN_TIERS[0]);
    let moved = |e: String| e.replace("order.service.ts", "orders.service.ts");
    assert_eq!(
        rows(&mb),
        [(
            format!(
                "http_model response | {ORDER_TS} | {ORDER_GO} | breaking modified client derived blocking=false note=-"
            ),
            order_exchanges().into_iter().map(moved).collect::<Vec<_>>(),
            strings(&[
                "trackingUrl added client json_field_missing breaking=true - -> string client=web/src/app/core/models/order.ts:17 server=-"
            ]),
        )],
        "the hit file moved: each exchange pairs through its route key: {:#?}",
        mb.rows
    );
    marker(
        &mb,
        "HEAD",
        "fact",
        "rows=1 breaking=1 blocking=0 advisory=1 compatible=0",
    );
}

#[test]
fn a_removed_document_loses_its_exchanges() {
    let repo = model_shop::renamed();
    repo.remove("admin/src/queries.ts");
    let mb = breaks(&repo, "HEAD", MODEL_MIN_TIERS[0]);
    assert!(mb.rows.is_empty(), "{:#?}", mb.rows);
    assert_eq!(
        mb.marker(),
        "[model-breaks] base=HEAD min_tier=fact exchanges=5 http=5 graphql=0 rows=0 breaking=0 \
         blocking=0 advisory=0 compatible=0 new_pairings=0 lost=2 unjudged=0",
        "a lost pairing is counted, never listed"
    );
    assert_eq!(
        mb.absence.as_ref().map(|a| a.note.as_str()),
        Some(
            "5 client <-> server model exchanges judged vs HEAD, none changed what a client reads or sends"
        )
    );
}
