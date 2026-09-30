//! CC.8a acceptance: contract breaks of the working tree against a git rev
//! (`glia_engine::contract_breaks`). Every schema copy and contract op is
//! paired old -> new and judged by its format's EVOLUTION rules (the before
//! side is the old schema, the after side the new one), and every client the
//! change left without a provider is listed. Each case is a real git repo
//! built through the shared two-commit harness.
//!
//! CC.8c: `contract_breaks_vs_rev_with` builds the rev pair as two multi-repo
//! merges (the provider at the rev and at its working tree, each with the
//! client repos), so a client in ANOTHER repo that loses its provider is an
//! orphan too.

mod git_fixture;

use git_fixture::GitRepo;
use glia_engine::contract_breaks::{
    ContractBreakArgs, ContractBreaks, SchemaChange, contract_breaks_vs_rev,
    contract_breaks_vs_rev_with,
};
use glia_engine::contract_fields::FieldChange;

/// The rationale's spec: one GET op whose 200 response declares `id`, `total`.
/// `get:` sits on line 7.
const ORDERS_V1: &str = "openapi: 3.0.0
info:
  title: orders
  version: \"1\"
paths:
  /orders/{id}:
    get:
      operationId: getOrder
      responses:
        \"200\":
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  id:
                    type: string
                  total:
                    type: number
";

fn breaks(repo: &GitRepo, args: &ContractBreakArgs) -> ContractBreaks {
    contract_breaks_vs_rev(repo.path(), "HEAD", args)
        .unwrap_or_else(|e| panic!("contract breaks vs HEAD: {e}"))
}

fn backward() -> ContractBreakArgs {
    ContractBreakArgs::default()
}

fn avro(mode: &'static str) -> ContractBreakArgs {
    let mut a = ContractBreakArgs::default();
    a.avro_mode = mode;
    a
}

fn dump(b: &ContractBreaks) -> String {
    let mut s = String::new();
    for r in &b.schemas {
        s.push_str(&format!(
            "  schema {} {} [{}] {} {} {} note={:?}\n",
            r.kind, r.key, r.format, r.status, r.change, r.tier, r.note
        ));
        for c in &r.changes {
            s.push_str(&format!(
                "    {} {} {} {:?} -> {:?} rule={} breaking={}\n",
                c.section, c.field, c.change, c.producer, c.consumer, c.rule, c.breaking
            ));
        }
    }
    for o in &b.orphaned_clients {
        s.push_str(&format!(
            "  orphan {} {} -> {} {} {} ({:?}:{:?})\n",
            o.category, o.client_qname, o.target_qname, o.reason, o.tier, o.file, o.line
        ));
    }
    s
}

fn only(b: &ContractBreaks) -> &SchemaChange {
    assert_eq!(b.schemas.len(), 1, "exactly one schema row:\n{}", dump(b));
    &b.schemas[0]
}

fn change<'a>(s: &'a SchemaChange, field: &str) -> &'a FieldChange {
    s.changes
        .iter()
        .find(|c| c.field == field)
        .unwrap_or_else(|| {
            panic!(
                "a change on `{field}` in {:?}",
                s.changes.iter().map(|c| &c.field).collect::<Vec<_>>()
            )
        })
}

#[test]
fn openapi_response_field_removed() {
    let repo = GitRepo::init();
    repo.write("openapi.yaml", ORDERS_V1);
    repo.commit("v1");
    repo.write(
        "openapi.yaml",
        &ORDERS_V1.replace(
            "                  total:\n                    type: number\n",
            "",
        ),
    );
    let b = breaks(&repo, &backward());
    let s = only(&b);
    assert_eq!(s.kind, "operation");
    assert_eq!(s.key, "GET /orders/{id}");
    assert_eq!(s.format, "openapi");
    assert_eq!(
        (s.status, s.change, s.tier),
        ("breaking", "modified", "fact"),
        "{}",
        dump(&b)
    );
    assert_eq!(s.note, None);
    let (before, after) = (
        s.before.as_ref().expect("before side"),
        s.after.as_ref().expect("after side"),
    );
    assert_eq!(before.qname, "contract::openapi::GET:/orders/{id}");
    assert_eq!(after.qname, before.qname);
    assert_eq!(after.file.as_deref(), Some("openapi.yaml"));
    assert_eq!(after.line, Some(7), "the op's `get:` line, 1-based");
    assert_eq!(s.changes.len(), 1, "{}", dump(&b));
    let c = &s.changes[0];
    assert_eq!(
        (c.section.as_str(), c.field.as_str(), c.change),
        ("response:200", "total", "producer_only")
    );
    assert_eq!(
        (c.producer.as_deref(), c.consumer.as_deref()),
        (Some("number"), None)
    );
    assert_eq!((c.rule, c.breaking), ("response_field_removed", true));
    assert_eq!(b.breaking, 1);
    assert_eq!(b.base, "HEAD");
    assert!(
        b.orphaned_clients.is_empty() && b.absence.is_none(),
        "{}",
        dump(&b)
    );
}

#[test]
fn openapi_response_field_added_is_compatible_and_clean_tree_lists_nothing() {
    let repo = GitRepo::init();
    repo.write("openapi.yaml", ORDERS_V1);
    repo.commit("v1");
    let clean = breaks(&repo, &backward());
    assert!(
        clean.schemas.is_empty() && clean.breaking == 0,
        "identical pairs are counted, not rowed:\n{}",
        dump(&clean)
    );
    let a = clean.absence.as_ref().expect("an empty answer says why");
    assert_eq!((a.tier, a.reason), ("FACT", "no_match"));

    repo.write(
        "openapi.yaml",
        &format!("{ORDERS_V1}                  currency:\n                    type: string\n"),
    );
    let b = breaks(&repo, &backward());
    let s = only(&b);
    assert_eq!(
        (s.status, s.change),
        ("compatible", "modified"),
        "{}",
        dump(&b)
    );
    let c = change(s, "currency");
    assert_eq!(
        (c.change, c.rule, c.breaking),
        ("consumer_only", "response_field_added", false)
    );
    assert_eq!(b.breaking, 0);
    let only_breaking = {
        let mut a = ContractBreakArgs::default();
        a.breaking_only = true;
        breaks(&repo, &a)
    };
    assert!(
        only_breaking.schemas.is_empty(),
        "breaking_only drops compatible rows:\n{}",
        dump(&only_breaking)
    );
}

const CREATE_V1: &str = "openapi: 3.0.0
info:
  title: orders
  version: \"1\"
paths:
  /orders:
    post:
      operationId: createOrder
      requestBody:
        content:
          application/json:
            schema:
              type: object
              required: [sku]
              properties:
                sku:
                  type: string
      responses:
        \"201\":
          description: created
";

fn create_with_qty(required: &str) -> String {
    CREATE_V1
        .replace("required: [sku]", required)
        .replace("                sku:\n                  type: string\n", "                sku:\n                  type: string\n                qty:\n                  type: integer\n")
}

#[test]
fn openapi_new_required_request_field() {
    let repo = GitRepo::init();
    repo.write("openapi.yaml", CREATE_V1);
    repo.commit("v1");

    repo.write("openapi.yaml", &create_with_qty("required: [sku, qty]"));
    let b = breaks(&repo, &backward());
    let s = only(&b);
    assert_eq!(
        (s.key.as_str(), s.status),
        ("POST /orders", "breaking"),
        "{}",
        dump(&b)
    );
    let c = change(s, "qty");
    assert_eq!((c.section.as_str(), c.change), ("request", "consumer_only"));
    assert_eq!(
        (c.producer.as_deref(), c.consumer.as_deref()),
        (None, Some("integer"))
    );
    assert_eq!((c.rule, c.breaking), ("new_required_request_field", true));
    assert_eq!(b.breaking, 1);

    repo.write("openapi.yaml", &create_with_qty("required: [sku]"));
    let b = breaks(&repo, &backward());
    let s = only(&b);
    assert_eq!(
        s.status,
        "compatible",
        "an optional request field breaks no client:\n{}",
        dump(&b)
    );
    assert!(s.changes.iter().all(|c| !c.breaking), "{}", dump(&b));
    assert_eq!(change(s, "qty").rule, "request_field_added");
    assert_eq!(b.breaking, 0);

    // An optional field already there that becomes required also breaks.
    repo.write("openapi.yaml", &create_with_qty("required: [sku]"));
    repo.commit("v2 optional qty");
    repo.write("openapi.yaml", &create_with_qty("required: [sku, qty]"));
    let b = breaks(&repo, &backward());
    let c = change(only(&b), "qty");
    assert_eq!(
        (c.change, c.rule, c.breaking),
        ("required", "new_required_request_field", true),
        "{}",
        dump(&b)
    );
}

#[test]
fn op_removed_is_breaking_and_op_added_is_compatible() {
    let repo = GitRepo::init();
    repo.write("openapi.yaml", ORDERS_V1);
    repo.commit("v1");
    // The GET op is gone; a POST op arrives.
    repo.write("openapi.yaml", CREATE_V1);
    let b = breaks(&repo, &backward());
    assert_eq!(b.schemas.len(), 2, "{}", dump(&b));
    // Sorted breaking first.
    let (gone, new) = (&b.schemas[0], &b.schemas[1]);
    assert_eq!(
        (gone.key.as_str(), gone.change, gone.status, gone.tier),
        ("GET /orders/{id}", "removed", "breaking", "fact")
    );
    assert!(gone.before.is_some() && gone.after.is_none());
    assert_eq!(
        gone.before.as_ref().and_then(|s| s.line),
        Some(7),
        "located in the rev's file"
    );
    assert_eq!(
        (new.key.as_str(), new.change, new.status, new.tier),
        ("POST /orders", "added", "compatible", "fact")
    );
    assert!(new.before.is_none() && new.after.is_some());
    assert_eq!(b.breaking, 1);
}

const SHOP_PROTO: &str = "syntax = \"proto3\";
package shop;

message Order {
  string id = 1;
  int64 total = 2;
}
";

#[test]
fn proto_field_type_change_and_unreserved_removal() {
    let repo = GitRepo::init();
    repo.write("shop.proto", SHOP_PROTO);
    repo.commit("v1");

    repo.write(
        "shop.proto",
        &SHOP_PROTO.replace("int64 total = 2;", "string total = 2;"),
    );
    let b = breaks(&repo, &backward());
    let s = only(&b);
    assert_eq!(
        (s.kind, s.key.as_str(), s.format.as_str()),
        ("message", "shop.Order", "proto")
    );
    assert_eq!(
        (s.status, s.change, s.tier),
        ("breaking", "modified", "fact"),
        "{}",
        dump(&b)
    );
    let c = change(s, "total");
    assert_eq!(
        (c.change, c.rule, c.breaking),
        ("type", "proto_wire_type", true)
    );
    assert_eq!(
        (c.producer.as_deref(), c.consumer.as_deref()),
        (Some("int64"), Some("string"))
    );

    // Dropped without `reserved 2;`: old readers survive it, but the number
    // is free for a later incompatible reuse.
    repo.write(
        "shop.proto",
        &SHOP_PROTO.replace("  int64 total = 2;\n", ""),
    );
    let b = breaks(&repo, &backward());
    let s = only(&b);
    assert_eq!(
        (s.status, s.note),
        ("compatible", Some("field_removed_unreserved")),
        "{}",
        dump(&b)
    );
    let c = change(s, "total");
    assert_eq!(
        (c.change, c.rule, c.breaking),
        ("producer_only", "field_removed_unreserved", false)
    );
    assert_eq!(c.producer.as_deref(), Some("int64"));

    // Dropped and reserved: the documented way to retire a field.
    repo.write(
        "shop.proto",
        &SHOP_PROTO.replace("  int64 total = 2;\n", "  reserved 2;\n"),
    );
    let b = breaks(&repo, &backward());
    let s = only(&b);
    assert_eq!((s.status, s.note), ("compatible", None), "{}", dump(&b));
    assert_eq!(change(s, "total").rule, "field_removed_reserved");
}

#[test]
fn proto_reserved_number_reused_is_breaking() {
    let repo = GitRepo::init();
    repo.write(
        "shop.proto",
        &SHOP_PROTO.replace("  int64 total = 2;\n", "  reserved 2;\n"),
    );
    repo.commit("v1");
    repo.write(
        "shop.proto",
        &SHOP_PROTO.replace("int64 total = 2;", "string note = 2;"),
    );
    let b = breaks(&repo, &backward());
    let c = change(only(&b), "note");
    assert_eq!(
        (c.change, c.rule, c.breaking),
        ("reserved", "proto_reserved_reused", true),
        "{}",
        dump(&b)
    );
}

const ORDER_AVSC: &str = r#"{"type": "record", "name": "Order", "namespace": "shop.avro", "fields": [{"name": "id", "type": "string"}]}
"#;

#[test]
fn avro_modes() {
    let repo = GitRepo::init();
    repo.write("order.avsc", ORDER_AVSC);
    repo.commit("v1");
    repo.write(
        "order.avsc",
        &ORDER_AVSC.replace(r#"}]}"#, r#"}, {"name": "total", "type": "long"}]}"#),
    );

    // Backward: the new schema reads data the old one wrote; `total` has no default.
    let b = breaks(&repo, &avro("backward"));
    let s = only(&b);
    assert_eq!(
        (s.kind, s.key.as_str(), s.format.as_str()),
        ("message", "shop.avro.Order", "avro")
    );
    assert_eq!(s.status, "breaking", "{}", dump(&b));
    let c = change(s, "total");
    assert_eq!(
        (c.change, c.rule, c.breaking),
        ("consumer_only", "avro_reader_field_no_default", true)
    );
    assert_eq!(
        (c.producer.as_deref(), c.consumer.as_deref()),
        (None, Some("long"))
    );

    // Forward: the old schema reads data the new one writes and ignores `total`.
    let b = breaks(&repo, &avro("forward"));
    let s = only(&b);
    assert_eq!(s.status, "compatible", "{}", dump(&b));
    let c = change(s, "total");
    // Still oriented before -> after: only the after side declares it.
    assert_eq!(
        (c.change, c.rule, c.breaking),
        ("consumer_only", "avro_writer_field_ignored", false)
    );
    assert_eq!(
        (c.producer.as_deref(), c.consumer.as_deref()),
        (None, Some("long"))
    );

    // Full: breaking either way breaks.
    let b = breaks(&repo, &avro("full"));
    let s = only(&b);
    assert_eq!(s.status, "breaking", "{}", dump(&b));
    let rules: Vec<&str> = s.changes.iter().map(|c| c.rule).collect();
    assert_eq!(
        rules,
        ["avro_reader_field_no_default", "avro_writer_field_ignored"],
        "{}",
        dump(&b)
    );

    // With a default, backward holds too.
    repo.write(
        "order.avsc",
        &ORDER_AVSC.replace(
            r#"}]}"#,
            r#"}, {"name": "total", "type": "long", "default": 0}]}"#,
        ),
    );
    let b = breaks(&repo, &avro("backward"));
    let s = only(&b);
    assert_eq!(
        (s.status, change(s, "total").rule),
        ("compatible", "avro_reader_field_default"),
        "{}",
        dump(&b)
    );

    assert!(
        contract_breaks_vs_rev(repo.path(), "HEAD", &avro("sideways")).is_err(),
        "an unknown mode is an error"
    );
}

const CLIENT_PY: &str =
    "import requests\n\n\ndef list_orders():\n    return requests.get(\"http://api/orders\")\n";
const APP_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/orders\")\ndef orders():\n    return []\n";

#[test]
fn route_removed_orphans_its_client() {
    let repo = GitRepo::init();
    repo.write("web/pyproject.toml", "[project]\nname = \"web\"\n");
    repo.write("web/client.py", CLIENT_PY);
    repo.write("services/api/pyproject.toml", "[project]\nname = \"api\"\n");
    repo.write("services/api/app.py", APP_PY);
    repo.commit("client + service");
    let clean = breaks(&repo, &backward());
    assert!(
        clean.orphaned_clients.is_empty(),
        "the committed client is served:\n{}",
        dump(&clean)
    );

    repo.remove("services/api/app.py");
    let b = breaks(&repo, &backward());
    assert!(b.schemas.is_empty(), "{}", dump(&b));
    assert_eq!(b.orphaned_clients.len(), 1, "{}", dump(&b));
    let o = &b.orphaned_clients[0];
    assert_eq!(o.client_qname, "endpoint:GET:/orders @web");
    assert_eq!(o.category, "HTTP_CALLS");
    assert_eq!(o.target_qname, "GET /orders @services/api");
    assert_eq!((o.reason, o.tier), ("target_removed", "fact"));
    assert_eq!(
        o.file.as_deref(),
        Some("web/client.py"),
        "located in the working tree"
    );
    assert_eq!(o.line, Some(5));
    assert_eq!(b.breaking, 1);
    assert!(b.absence.is_none());
}

#[test]
fn removed_client_is_not_an_orphan() {
    let repo = GitRepo::init();
    repo.write("web/pyproject.toml", "[project]\nname = \"web\"\n");
    repo.write("web/client.py", CLIENT_PY);
    repo.write("services/api/pyproject.toml", "[project]\nname = \"api\"\n");
    repo.write("services/api/app.py", APP_PY);
    repo.commit("client + service");
    repo.remove("web/client.py");
    let b = breaks(&repo, &backward());
    assert!(
        b.orphaned_clients.is_empty(),
        "a client that left with its call orphans nothing:\n{}",
        dump(&b)
    );
    assert_eq!(b.breaking, 0);
}

#[test]
fn no_contracts_absence() {
    let repo = GitRepo::init();
    repo.write("shop/a.py", "def place(o):\n    return o\n");
    repo.commit("shop");
    repo.write("shop/a.py", "def place(o):\n    return o + 1\n");
    let b = breaks(&repo, &backward());
    assert!(
        b.schemas.is_empty() && b.orphaned_clients.is_empty(),
        "{}",
        dump(&b)
    );
    assert_eq!(b.breaking, 0);
    let a = b
        .absence
        .as_ref()
        .expect("no contract on either side: an absence");
    assert_eq!((a.tier, a.reason), ("FACT", "no_match"));
    assert!(a.note.contains("no contract"), "{}", a.note);
}

/// CC.8c: the provider is a git repo, the client a separate plain dir (no
/// git) built beside it. At the rev and in the working tree alike the client
/// is built at its own tree, so its NodeIds match across the pair and only
/// the provider's change moves an edge: deleting the route leaves the
/// other repo's client orphaned. Without `--with` the client is invisible.
#[test]
fn client_in_another_repo_is_orphaned() {
    let provider = GitRepo::init();
    provider.write("services/api/pyproject.toml", "[project]\nname = \"api\"\n");
    provider.write("services/api/app.py", APP_PY);
    // An unchanged contract: the rev side is built under the working tree's
    // identity, so it pairs with itself and lists nothing. Built under the
    // temp dir's own identity it would come out removed + added.
    provider.write("services/api/openapi.yaml", ORDERS_V1);
    provider.commit("service");
    let client_dir = tempfile::tempdir().expect("temp dir for the client repo");
    let web = client_dir.path().join("web");
    std::fs::create_dir_all(&web).expect("client web dir");
    std::fs::write(web.join("client.py"), CLIENT_PY).expect("client write");
    let clients = [client_dir.path().to_str().expect("utf-8 temp path").to_string()];

    let clean = contract_breaks_vs_rev_with(provider.path(), "HEAD", &clients, &backward())
        .unwrap_or_else(|e| panic!("contract breaks --with vs HEAD: {e}"));
    assert!(
        clean.orphaned_clients.is_empty() && clean.schemas.is_empty(),
        "the committed route serves the other repo's client:\n{}",
        dump(&clean)
    );
    assert_eq!(clean.breaking, 0);
    assert!(clean.absence.is_some(), "an empty answer says why");

    provider.remove("services/api/app.py");
    let b = contract_breaks_vs_rev_with(provider.path(), "HEAD", &clients, &backward())
        .unwrap_or_else(|e| panic!("contract breaks --with vs HEAD: {e}"));
    assert!(b.schemas.is_empty(), "{}", dump(&b));
    assert_eq!(b.orphaned_clients.len(), 1, "{}", dump(&b));
    let o = &b.orphaned_clients[0];
    assert_eq!(
        o.client_qname, "endpoint:GET:/orders",
        "the merged build's client name: the client repo has no project root"
    );
    assert_eq!(o.category, "HTTP_CALLS");
    assert_eq!(o.target_qname, "GET /orders @services/api");
    assert_eq!((o.reason, o.tier), ("target_removed", "fact"));
    assert_eq!(
        (o.file.as_deref(), o.line),
        (Some("web/client.py"), Some(5)),
        "located in the client repo's tree, 1-based"
    );
    assert_eq!(b.breaking, 1);
    assert_eq!(b.base, "HEAD");
    assert!(b.absence.is_none());

    let alone = breaks(&provider, &backward());
    assert!(
        alone.orphaned_clients.is_empty(),
        "built alone, the provider has no client to orphan:\n{}",
        dump(&alone)
    );
    assert_eq!(alone.breaking, 0);

    let err = contract_breaks_vs_rev_with(provider.path(), "HEAD", &clients, &avro("sideways"))
        .expect_err("an unknown mode is refused before any build");
    assert!(err.contains("sideways"), "{err}");
}

/// CC.8c: a client repo that is itself a git work tree is built at its
/// working tree on BOTH sides: only the provider moves. The client's call is
/// uncommitted, so rolling the client back to its own HEAD would hide it;
/// built as it stands, it loses its provider. Both fixtures are `<tmp>/repo`
/// checkouts without a remote, so they share the identity key `gitdir:repo`
/// and are disambiguated by path, the same way on both sides.
#[test]
fn client_git_work_tree_is_not_rolled_back() {
    let provider = GitRepo::init();
    provider.write("services/api/pyproject.toml", "[project]\nname = \"api\"\n");
    provider.write("services/api/app.py", APP_PY);
    provider.commit("service");
    let client = GitRepo::init();
    client.write("web/README.md", "# web\n");
    client.commit("client skeleton");
    client.write("web/client.py", CLIENT_PY);
    let clients = [client.path().to_string()];
    provider.remove("services/api/app.py");
    let b = contract_breaks_vs_rev_with(provider.path(), "HEAD", &clients, &backward())
        .unwrap_or_else(|e| panic!("contract breaks --with vs HEAD: {e}"));
    assert!(b.schemas.is_empty(), "{}", dump(&b));
    assert_eq!(
        b.orphaned_clients.len(),
        1,
        "the client's uncommitted call is on both sides and loses its provider:\n{}",
        dump(&b)
    );
    let o = &b.orphaned_clients[0];
    assert_eq!(
        (o.client_qname.as_str(), o.target_qname.as_str(), o.reason),
        ("endpoint:GET:/orders", "GET /orders @services/api", "target_removed")
    );
    assert_eq!(b.breaking, 1);
}
