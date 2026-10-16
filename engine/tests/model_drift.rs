//! DT.1i acceptance: model drift in `contract_fields`, the two pairings over
//! the models pass (DT.1g's client wires, DT.1m's wire compare).
//!
//! - `http_model`: each client call site against the handler of the route it
//!   is paired with, per direction, DT.1m's `WireChange`s mapped onto the
//!   field vocabulary (`producer_only` / `consumer_only` / `required` /
//!   `type`), one row per (route, direction, client model or call site,
//!   server model or handler). Over `models-http-shop` (DT.1g's fixture,
//!   read only) these are the eleven pairs of `model_wire_pairs.rs`.
//! - `client_contract`: a typed client call site against the OpenAPI /
//!   swagger op that DOCUMENTS its route, by the op's flattened names. Over
//!   `models-http-contract` the swagger.json op is compared and the swaggo
//!   `@Router` op documenting the same route is not (it carries no
//!   SCHEMA_FIELDS).
//!
//! The trees are committed substrate-gap fixtures built with `generate_many`
//! over the dirs their key.json names; the variant is written to a temp dir.
//!
//! fired_on: `[contract-fields] pairs=.. http_model=<n> client_contract=<n>`
//! (after `[models] pairs=..`), printed by `contract_fields`:
//! `cargo test -p glia-engine --test model_drift -- --nocapture 2>&1 | grep
//! '^\[contract-fields\]'`.

use std::path::{Path, PathBuf};

use glia_engine::contract_fields::{FieldChange, FieldDiffRow, contract_fields};
use glia_engine::generate_many;

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

fn rows_for(paths: &[String]) -> Vec<FieldDiffRow> {
    contract_fields(&generate_many(paths).expect("the fixture builds").merged)
}

/// Copy a fixture's files into `dst` so a variant can edit them.
fn copy_tree(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            if entry.file_name() == ".ai" || entry.file_name() == ".glia" {
                continue;
            }
            std::fs::create_dir_all(&to).unwrap();
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}

type Change = (
    String,
    String,
    &'static str,
    Option<String>,
    Option<String>,
    &'static str,
    bool,
);

fn changes(r: &FieldDiffRow) -> Vec<Change> {
    r.changes
        .iter()
        .map(|c: &FieldChange| {
            (
                c.section.clone(),
                c.field.clone(),
                c.change,
                c.producer.clone(),
                c.consumer.clone(),
                c.rule,
                c.breaking,
            )
        })
        .collect()
}

fn ch(
    section: &str,
    field: &str,
    change: &'static str,
    (producer, consumer): (Option<&str>, Option<&str>),
    rule: &'static str,
    breaking: bool,
) -> Change {
    (
        section.to_string(),
        field.to_string(),
        change,
        producer.map(str::to_string),
        consumer.map(str::to_string),
        rule,
        breaking,
    )
}

/// `(key, producer qname, consumer qname, status, note, changes)`.
type RowView = (
    String,
    String,
    String,
    &'static str,
    Option<&'static str>,
    Vec<Change>,
);

fn view(rows: &[FieldDiffRow], pairing: &str) -> Vec<RowView> {
    rows.iter()
        .filter(|r| r.pairing == pairing)
        .map(|r| {
            (
                r.key.clone(),
                r.producer.qname.clone(),
                r.consumer.qname.clone(),
                r.status,
                r.note,
                changes(r),
            )
        })
        .collect()
}

fn rv(
    key: &str,
    producer: &str,
    consumer: &str,
    status: &'static str,
    changes: Vec<Change>,
) -> RowView {
    (
        key.to_string(),
        producer.to_string(),
        consumer.to_string(),
        status,
        None,
        changes,
    )
}

fn only<'a>(rows: &'a [FieldDiffRow], pairing: &str, key: &str) -> &'a FieldDiffRow {
    let of: Vec<&FieldDiffRow> = rows
        .iter()
        .filter(|r| r.pairing == pairing && r.key == key)
        .collect();
    assert_eq!(of.len(), 1, "one {pairing} row `{key}`: {rows:#?}");
    of[0]
}

const S: Option<&str> = Some("string");
const N: Option<&str> = None;

/// The Invoice response's drift (DT.1m): the client reads `customerName`,
/// the server writes `customer_name`, the optional `couponCode` is never
/// written; `prefix` is the envelope path holding it.
fn invoice(prefix: &str) -> Vec<Change> {
    let f = |n: &str| format!("{prefix}{n}");
    vec![
        ch(
            "response",
            &f("couponCode"),
            "consumer_only",
            (N, S),
            "json_field_optional_absent",
            false,
        ),
        ch(
            "response",
            &f("customerName"),
            "consumer_only",
            (N, S),
            "json_field_missing",
            true,
        ),
        ch(
            "response",
            &f("customer_name"),
            "producer_only",
            (S, N),
            "json_field_unread",
            false,
        ),
    ]
}

#[test]
fn http_model_rows_over_the_shop_fixture() {
    let rows = rows_for(&fixture_dirs("models-http-shop"));
    for r in &rows {
        assert_eq!(
            r.pairing, "http_model",
            "no contract op documents a shop route: {r:#?}"
        );
        assert_eq!(r.tier, "derived");
    }
    let tsi = "src::app::models::invoice::Invoice";
    let goi = "models::invoice::Invoice";
    let expected = vec![
        rv(
            "GET /api/articles response",
            "models::catalog::CatalogEntry",
            "src::app::models::catalog::Article",
            "identical",
            vec![],
        ),
        rv(
            "GET /api/invoice-pages/:n response",
            "handlers::handlers::Handlers::InvoicePage",
            "endpoint:GET:/api/invoice-pages/${…}",
            "breaking",
            invoice("invoices[]."),
        ),
        rv(
            "GET /api/invoices/:id response",
            goi,
            tsi,
            "breaking",
            invoice(""),
        ),
        rv(
            "GET /api/listings response",
            "handlers::handlers::Handlers::Listings",
            "endpoint:GET:/api/listings",
            "identical",
            vec![],
        ),
        rv(
            "GET /api/listings/:id response",
            "models::catalog::Listing",
            "src::app::models::catalog::Listing",
            "compatible",
            vec![ch(
                "response",
                "sellerName",
                "consumer_only",
                (N, S),
                "json_field_optional_absent",
                false,
            )],
        ),
        rv(
            "GET /api/summary/daily response",
            "models::catalog::DailySummary",
            "src::app::models::catalog::Summary",
            "compatible",
            vec![ch(
                "response",
                "day",
                "producer_only",
                (S, N),
                "json_field_unread",
                false,
            )],
        ),
        rv(
            "GET /api/summary/weekly response",
            "models::catalog::WeeklySummary",
            "src::app::models::catalog::Summary",
            "compatible",
            vec![ch(
                "response",
                "week",
                "producer_only",
                (Some("int"), N),
                "json_field_unread",
                false,
            )],
        ),
        rv(
            "PATCH /api/invoices/:id/status request",
            "endpoint:PATCH:/api/invoices/${…}/status",
            "models::invoice::StatusUpdate",
            "identical",
            vec![],
        ),
        rv(
            "PATCH /api/invoices/:id/status response",
            goi,
            tsi,
            "breaking",
            invoice(""),
        ),
        rv(
            "POST /api/invoices request",
            "src::app::models::invoice::NewInvoice",
            "models::invoice::CreateInvoiceRequest",
            "compatible",
            vec![ch(
                "request",
                "couponCode",
                "producer_only",
                (S, N),
                "json_field_dropped",
                false,
            )],
        ),
        rv(
            "POST /api/invoices response",
            goi,
            tsi,
            "breaking",
            invoice(""),
        ),
    ];
    assert_eq!(view(&rows, "http_model"), expected, "{rows:#?}");
    assert!(
        rows.iter()
            .flat_map(|r| &r.changes)
            .all(|c| !c.field.contains("Note")),
        "json:\"-\" is no change"
    );

    // An inline client type is located at its call site, an inline server
    // envelope at the handler's write; a model at its declaration, in its
    // schema's format.
    let page = only(&rows, "http_model", "GET /api/invoice-pages/:n response");
    let at = |s: &glia_engine::contract_fields::FieldSide| {
        (
            s.format.clone(),
            s.file.clone().unwrap_or_default(),
            s.line.unwrap_or(0),
        )
    };
    assert_eq!(
        at(&page.producer),
        ("go".to_string(), "handlers/handlers.go".to_string(), 71)
    );
    assert_eq!(
        at(&page.consumer),
        (
            "ts".to_string(),
            "src/app/services/invoice.service.ts".to_string(),
            25
        )
    );
    let get = only(&rows, "http_model", "GET /api/invoices/:id response");
    assert_eq!(
        at(&get.producer),
        ("go".to_string(), "models/invoice.go".to_string(), 11)
    );
    assert_eq!(
        at(&get.consumer),
        ("ts".to_string(), "src/app/models/invoice.ts".to_string(), 9)
    );
    assert_ne!(get.producer.repo_id, get.consumer.repo_id, "{get:#?}");
}

#[test]
fn client_contract_and_http_model_over_the_contract_fixture() {
    let rows = rows_for(&fixture_dirs("models-http-contract"));
    let plan = "src::app::plan.service::Plan";
    let swagger = "contract::docs::swagger::GET:/api/plans/{id}";
    assert_eq!(
        view(&rows, "client_contract"),
        vec![rv(
            "GET /api/plans/{id} response",
            swagger,
            plan,
            "breaking",
            vec![ch(
                "response:200",
                "trialDays",
                "consumer_only",
                (N, Some("number")),
                "contract_field_undeclared",
                true,
            )],
        )],
        "{rows:#?}"
    );
    assert_eq!(
        view(&rows, "http_model"),
        vec![rv(
            "GET /api/plans/:id response",
            "handlers::plans::planBody",
            plan,
            "breaking",
            vec![ch(
                "response",
                "trialDays",
                "consumer_only",
                (N, Some("number")),
                "json_field_missing",
                true,
            )],
        )],
        "{rows:#?}"
    );
    assert_eq!(rows.len(), 2, "{rows:#?}");
    // The swaggo op documents the route too, but declares no fields.
    assert!(
        rows.iter()
            .all(|r| r.producer.qname != "contract::handlers::plans::GET:/api/plans/{id}"),
        "{rows:#?}"
    );
    let op = &only(&rows, "client_contract", "GET /api/plans/{id} response").producer;
    assert_eq!(
        (op.format.as_str(), op.file.as_deref()),
        ("openapi", Some("docs/swagger.json"))
    );
}

const VARIANT_TS: &str = r#"import { HttpClient } from '@angular/common/http';
import { Injectable } from '@angular/core';

export interface Tier {
  sku: string;
  seats: number;
}

export interface Plan {
  id: string;
  name: string;
  priceCents: number;
  trialDays: number;
  tiers: Tier[];
}

export interface NewPlan {
  name: string;
  priceCents: string;
  note?: string;
}

@Injectable({ providedIn: 'root' })
export class PlanService {
  constructor(private http: HttpClient) {}

  plan(id: string) {
    return this.http.get<Plan>(`/api/plans/${id}`);
  }

  again(id: string) {
    return this.http.get<Plan>(`/api/plans/${id}`);
  }

  list() {
    return this.http.get<Plan[]>('/api/plans');
  }

  create(body: NewPlan) {
    return this.http.post<Plan>('/api/plans', body);
  }

  raw(id: string) {
    return this.http.get<any>(`/api/plans/${id}/raw`);
  }
}
"#;

const VARIANT_GO: &str = r#"package handlers

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

type tierBody struct {
	SKU   string `json:"sku"`
	Seats int    `json:"seats"`
}

type planBody struct {
	ID         string     `json:"id"`
	Name       string     `json:"name"`
	PriceCents int64      `json:"priceCents"`
	Tiers      []tierBody `json:"tiers"`
}

type createPlan struct {
	Name       string `json:"name" binding:"required"`
	PriceCents int64  `json:"priceCents" binding:"required"`
	Currency   string `json:"currency" binding:"required"`
}

func Register(r *gin.Engine) {
	r.GET("/api/plans/:id", GetPlan)
	r.GET("/api/plans", ListPlans)
	r.POST("/api/plans", CreatePlan)
	r.GET("/api/plans/:id/raw", RawPlan)
}

func GetPlan(c *gin.Context) {
	c.JSON(http.StatusOK, planBody{ID: c.Param("id"), Name: "basic", PriceCents: 900})
}

func ListPlans(c *gin.Context) {
	c.JSON(http.StatusOK, []planBody{})
}

func CreatePlan(c *gin.Context) {
	var req createPlan
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	c.JSON(http.StatusCreated, planBody{Name: req.Name, PriceCents: req.PriceCents})
}

func RawPlan(c *gin.Context) {
	c.JSON(http.StatusOK, gin.H{"id": c.Param("id")})
}
"#;

const VARIANT_SWAGGER: &str = r##"{
  "swagger": "2.0",
  "info": {"title": "Billing API", "version": "1.0"},
  "paths": {
    "/api/plans/{id}": {
      "get": {
        "operationId": "getPlan",
        "parameters": [{"name": "id", "in": "path", "required": true, "type": "string"}],
        "responses": {"200": {"description": "OK", "schema": {"$ref": "#/definitions/Plan"}}}
      }
    },
    "/api/plans": {
      "get": {
        "operationId": "listPlans",
        "responses": {"200": {"description": "OK", "schema": {"type": "array", "items": {"$ref": "#/definitions/Plan"}}}}
      },
      "post": {
        "operationId": "createPlan",
        "parameters": [{"name": "body", "in": "body", "required": true, "schema": {"$ref": "#/definitions/CreatePlan"}}],
        "responses": {"201": {"description": "Created", "schema": {"$ref": "#/definitions/Plan"}}}
      }
    },
    "/api/plans/{id}/raw": {
      "get": {
        "operationId": "rawPlan",
        "parameters": [{"name": "id", "in": "path", "required": true, "type": "string"}],
        "responses": {"200": {"description": "OK", "schema": {"type": "object", "properties": {"id": {"type": "string"}}}}}
      }
    }
  },
  "definitions": {
    "Tier": {
      "type": "object",
      "properties": {"sku": {"type": "string"}, "seats": {"type": "string"}}
    },
    "Plan": {
      "type": "object",
      "required": ["id", "name", "priceCents"],
      "properties": {
        "id": {"type": "string"},
        "name": {"type": "string"},
        "priceCents": {"type": "integer"},
        "tiers": {"type": "array", "items": {"$ref": "#/definitions/Tier"}}
      }
    },
    "CreatePlan": {
      "type": "object",
      "required": ["name", "priceCents", "currency"],
      "properties": {
        "name": {"type": "string"},
        "priceCents": {"type": "integer"},
        "currency": {"type": "string"},
        "tags": {"type": "array", "items": {"type": "string"}}
      }
    }
  }
}
"##;

/// Every client_contract rule, array bodies and nested arrays, a request
/// both ways, two call sites reading one model (one row), and a root the
/// client never typed (`client_type_unresolved`, nothing compared).
#[test]
fn client_contract_rules_over_a_variant() {
    let tmp = tempfile::tempdir().unwrap();
    copy_tree(&fixture("models-http-contract"), tmp.path());
    let write = |rel: &str, body: &str| std::fs::write(tmp.path().join(rel), body).unwrap();
    write("portal/src/app/plan.service.ts", VARIANT_TS);
    write("billing/handlers/plans.go", VARIANT_GO);
    write("billing/docs/swagger.json", VARIANT_SWAGGER);
    let dirs: Vec<String> = ["portal", "billing"]
        .iter()
        .map(|d| tmp.path().join(d).to_str().unwrap().to_string())
        .collect();
    let rows = rows_for(&dirs);

    let plan = "src::app::plan.service::Plan";
    let new_plan = "src::app::plan.service::NewPlan";
    let op = |m: &str, p: &str| format!("contract::docs::swagger::{m}:{p}");
    let num = Some("number");
    let undeclared = |field: &str, sec: &str| {
        ch(
            sec,
            field,
            "consumer_only",
            (N, num),
            "contract_field_undeclared",
            true,
        )
    };
    let seats =
        |field: &str, sec: &str| ch(sec, field, "type", (S, num), "contract_type_mismatch", true);
    let mut expected = vec![
        rv(
            "GET /api/plans response",
            &op("GET", "/api/plans"),
            plan,
            "breaking",
            vec![
                seats("[].tiers[].seats", "response:200"),
                undeclared("[].trialDays", "response:200"),
            ],
        ),
        rv(
            "GET /api/plans/{id} response",
            &op("GET", "/api/plans/{id}"),
            plan,
            "breaking",
            vec![
                seats("tiers[].seats", "response:200"),
                undeclared("trialDays", "response:200"),
            ],
        ),
        rv(
            "GET /api/plans/{id}/raw response",
            &op("GET", "/api/plans/{id}/raw"),
            "endpoint:GET:/api/plans/${…}/raw",
            "unknown",
            vec![],
        ),
        rv(
            "POST /api/plans request",
            new_plan,
            &op("POST", "/api/plans"),
            "breaking",
            vec![
                ch(
                    "request",
                    "note",
                    "producer_only",
                    (S, N),
                    "contract_request_field_undeclared",
                    false,
                ),
                ch(
                    "request",
                    "priceCents",
                    "type",
                    (S, Some("integer")),
                    "contract_type_mismatch",
                    true,
                ),
                ch(
                    "request",
                    "currency",
                    "consumer_only",
                    (N, S),
                    "contract_required_missing",
                    true,
                ),
                ch(
                    "request",
                    "tags[]",
                    "consumer_only",
                    (N, S),
                    "contract_field_unset",
                    false,
                ),
            ],
        ),
        rv(
            "POST /api/plans response",
            &op("POST", "/api/plans"),
            plan,
            "breaking",
            vec![
                seats("tiers[].seats", "response:201"),
                undeclared("trialDays", "response:201"),
            ],
        ),
    ];
    expected[2].4 = Some("client_type_unresolved");
    assert_eq!(view(&rows, "client_contract"), expected, "{rows:#?}");

    // The request against the Go binder: the same exchange, judged by
    // encoding/json (the kinds and the binding:"required" tag).
    let req = only(&rows, "http_model", "POST /api/plans request");
    assert_eq!(
        (
            req.producer.qname.as_str(),
            req.consumer.qname.as_str(),
            req.status
        ),
        (new_plan, "handlers::plans::createPlan", "breaking")
    );
    assert_eq!(
        changes(req),
        vec![
            ch(
                "request",
                "currency",
                "consumer_only",
                (N, S),
                "json_required_missing",
                true
            ),
            ch(
                "request",
                "note",
                "producer_only",
                (S, N),
                "json_field_dropped",
                false
            ),
            ch(
                "request",
                "priceCents",
                "type",
                (S, Some("int64")),
                "json_type_mismatch",
                true
            ),
        ],
        "{req:#?}"
    );
    // Two call sites reading Plan from one route: one row each pairing.
    let get = only(&rows, "http_model", "GET /api/plans/:id response");
    assert_eq!(
        changes(get),
        vec![ch(
            "response",
            "trialDays",
            "consumer_only",
            (N, num),
            "json_field_missing",
            true
        )],
        "{get:#?}"
    );
    let raw = only(&rows, "http_model", "GET /api/plans/:id/raw response");
    assert_eq!(
        (raw.status, raw.note),
        ("unknown", Some("client_type_unresolved")),
        "{raw:#?}"
    );
    assert!(raw.changes.is_empty());
}

/// DT.1n (the DT.1m request gap): the shop with NewInvoice's `customerName`
/// made optional. CreateInvoiceRequest's gin binder requires it, so a body
/// without it is a 400: the request row gains a breaking `required` change,
/// rule `json_required_may_be_unset`.
#[test]
fn a_required_bind_the_client_may_omit_breaks_the_request_row() {
    let tmp = tempfile::tempdir().unwrap();
    copy_tree(&fixture("models-http-shop"), tmp.path());
    let ts = tmp.path().join("web/src/app/models/invoice.ts");
    let text = std::fs::read_to_string(&ts).unwrap();
    let old = "export interface NewInvoice {\n  customerName: string;";
    assert!(text.contains(old), "the fixture's NewInvoice moved");
    std::fs::write(&ts, text.replace(old, "export interface NewInvoice {\n  customerName?: string;")).unwrap();
    let dirs: Vec<String> = ["web", "api"]
        .iter()
        .map(|d| tmp.path().join(d).to_str().unwrap().to_string())
        .collect();
    assert_eq!(
        fixture_dirs("models-http-shop")
            .iter()
            .map(|d| Path::new(d).file_name().unwrap().to_str().unwrap().to_string())
            .collect::<Vec<_>>(),
        ["web", "api"],
        "the variant builds the fixture's dirs"
    );
    let rows = rows_for(&dirs);
    let req = only(&rows, "http_model", "POST /api/invoices request");
    assert_eq!(
        (req.status, req.note, changes(req)),
        (
            "breaking",
            None,
            vec![
                ch(
                    "request",
                    "couponCode",
                    "producer_only",
                    (S, N),
                    "json_field_dropped",
                    false,
                ),
                ch(
                    "request",
                    "customerName",
                    "required",
                    (S, S),
                    "json_required_may_be_unset",
                    true,
                ),
            ]
        ),
        "{req:#?}"
    );
}

#[test]
fn model_rows_are_deterministic() {
    let dirs = fixture_dirs("models-http-shop");
    let a = serde_json::to_string(&rows_for(&dirs)).unwrap();
    let b = serde_json::to_string(&rows_for(&dirs)).unwrap();
    assert_eq!(a, b);
}
