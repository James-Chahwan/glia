//! FD.80 — a gRPC server registration binds HANDLED_BY to the implementation
//! it names, never to the function that holds the registration.
//!
//! FD.3c's edge audit (HANDLED_BY|fact, clusters H1 + H2) found
//! `grpc_server:<S>` markers HANDLED_BY the bootstrap that registers them
//! (Node `loadAllProtos`, Python `start`, Go `run`) and a library servicer
//! (`health.NewServer()`) bound to a same-named app method. The fixture
//! `bench/substrate-gap/fixtures/grpc-register-impl-handlers` holds the
//! microservices-demo shapes; it is copied into a tempdir (`bench/` is outside
//! the cargo workspace's crates) and built there.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, node_kind};
use glia_core::NodeId;
use glia_engine::{GenerateResult, generate_one};

fn bench_fixture(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}/../bench/substrate-gap/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Copy a fixture's sources (never its key.json or a stray `.ai/` cache).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn build(dir: &Path) -> GenerateResult {
    generate_one(dir.to_str().unwrap()).expect("generate_one")
}

/// `(marker qname, target qname, EVIDENCE rule)` of every HANDLED_BY edge out
/// of a GRPC_SERVER, sorted and deduplicated.
fn handled_by(r: &GenerateResult) -> BTreeSet<(String, String, String)> {
    let qname = |id: NodeId| {
        r.merged
            .graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_default()
    };
    let is_server = |id: NodeId| {
        r.merged
            .graphs
            .iter()
            .any(|g| g.nav.kind_by_id.get(&id) == Some(&node_kind::GRPC_SERVER))
    };
    r.merged
        .all_edges()
        .filter(|e| e.category == edge_category::HANDLED_BY && is_server(e.from))
        .map(|e| {
            let rule = Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default();
            (qname(e.from), qname(e.to), rule)
        })
        .collect()
}

fn pairs(rows: &BTreeSet<(String, String, String)>) -> BTreeSet<(String, String)> {
    rows.iter()
        .map(|(f, t, _)| (f.clone(), t.clone()))
        .collect()
}

#[test]
fn registrations_bind_the_handlers_they_name_never_the_bootstrap() {
    let td = tempfile::tempdir().unwrap();
    copy_tree(&bench_fixture("grpc-register-impl-handlers"), td.path());
    let r = build(td.path());
    let got = pairs(&handled_by(&r));

    let expect = [
        (
            "grpc_server:PaymentService @src/paymentservice",
            "src::paymentservice::server::HipsterShopServer::ChargeServiceHandler",
        ),
        (
            "grpc_server:Health @src/paymentservice",
            "src::paymentservice::server::HipsterShopServer::CheckHandler",
        ),
        (
            "grpc_server:EmailService @src/emailservice",
            "src::emailservice::email_server::DummyEmailService::SendOrderConfirmation",
        ),
        (
            "grpc_server:Health @src/emailservice",
            "src::emailservice::email_server::BaseEmailService::Check",
        ),
        (
            "grpc_server:ProductCatalogService @src/productcatalogservice",
            "src::productcatalogservice::product_catalog::productCatalog::GetProduct",
        ),
        (
            "grpc_server:ShippingService @src/shippingservice",
            "src::shippingservice::main::server::GetQuote",
        ),
    ];
    for (from, to) in expect {
        assert!(
            got.contains(&(from.to_string(), to.to_string())),
            "missing {from} -HANDLED_BY-> {to}; got {got:#?}"
        );
    }
    let forbid = [
        (
            "grpc_server:PaymentService @src/paymentservice",
            "src::paymentservice::server::HipsterShopServer::loadAllProtos",
        ),
        (
            "grpc_server:Health @src/paymentservice",
            "src::paymentservice::server::HipsterShopServer::loadAllProtos",
        ),
        (
            "grpc_server:EmailService @src/emailservice",
            "src::emailservice::email_server::start",
        ),
        (
            "grpc_server:ProductCatalogService @src/productcatalogservice",
            "src::productcatalogservice::server::run",
        ),
        (
            "grpc_server:Health @src/productcatalogservice",
            "src::productcatalogservice::server::run",
        ),
        (
            "grpc_server:Health @src/shippingservice",
            "src::shippingservice::main::server::Check",
        ),
    ];
    for (from, to) in forbid {
        assert!(
            !got.contains(&(from.to_string(), to.to_string())),
            "forbidden {from} -HANDLED_BY-> {to}"
        );
    }
    // No marker is HANDLED_BY a registering function anywhere in the tree.
    for (from, to) in &got {
        assert!(
            !to.ends_with("::main") && !to.ends_with("::run") && !to.ends_with("::start"),
            "{from} -HANDLED_BY-> {to}"
        );
    }
}

const GREETER_PROTO: &str = "syntax = \"proto3\";\npackage hello;\noption go_package = \"example.com/hello/pb\";\n\nservice Greeter {\n  rpc SayHello (HelloRequest) returns (HelloReply);\n}\n";

const MAIN_GO: &str = "package main\n\nimport (\n\t\"net\"\n\n\t\"google.golang.org/grpc\"\n\tpb \"example.com/hello/pb\"\n)\n\nfunc main() {\n\tlis, _ := net.Listen(\"tcp\", \":50051\")\n\ts := grpc.NewServer()\n\tpb.RegisterGreeterServer(s, &greeterImpl{})\n\ts.Serve(lis)\n}\n";

const IMPL_GO: &str = "package main\n\nimport (\n\t\"context\"\n\n\tpb \"example.com/hello/pb\"\n)\n\n// greeterImpl embeds no Unimplemented base: only the registration names it.\ntype greeterImpl struct{}\n\nfunc (g *greeterImpl) SayHello(ctx context.Context, in *pb.HelloRequest) (*pb.HelloReply, error) {\n\treturn &pb.HelloReply{}, nil\n}\n\nfunc (g *greeterImpl) helper() {}\n";

#[test]
fn a_go_impl_type_in_another_file_of_the_package_binds_by_register_impl() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "api.proto", GREETER_PROTO);
    write(td.path(), "go.mod", "module example.com/hello\n\ngo 1.22\n");
    write(td.path(), "main.go", MAIN_GO);
    write(td.path(), "impl.go", IMPL_GO);
    let r = build(td.path());
    let rows = handled_by(&r);
    assert!(
        rows.contains(&(
            "grpc_server:Greeter".to_string(),
            "impl::greeterImpl::SayHello".to_string(),
            "register_impl".to_string(),
        )),
        "the impl type's rpc method, rule register_impl: {rows:#?}"
    );
    assert!(
        !rows.iter().any(|(_, to, _)| to == "main::main"),
        "the registering main is no handler: {rows:#?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(_, to, _)| to == "impl::greeterImpl::helper"),
        "only rpc methods bind: {rows:#?}"
    );
}

#[test]
fn a_registration_of_another_packages_servicer_binds_nothing_and_sits_in_its_module() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "health.proto",
        "syntax = \"proto3\";\npackage grpc.health.v1;\n\nservice Health {\n  rpc Check (A) returns (B);\n}\n",
    );
    write(td.path(), "go.mod", "module example.com/svc\n\ngo 1.22\n");
    write(
        td.path(),
        "main.go",
        "package main\n\nimport (\n\t\"google.golang.org/grpc\"\n\t\"google.golang.org/grpc/health\"\n\thealthpb \"google.golang.org/grpc/health/grpc_health_v1\"\n)\n\ntype app struct{}\n\nfunc (a *app) Check() {}\n\nfunc main() {\n\ts := grpc.NewServer()\n\thealthpb.RegisterHealthServer(s, health.NewServer())\n}\n",
    );
    let r = build(td.path());
    assert!(handled_by(&r).is_empty(), "{:#?}", handled_by(&r));
    // The marker still exists, located, and its MODULE contains it.
    let marker = r
        .merged
        .graphs
        .iter()
        .find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .find_map(|(id, q)| (q == "grpc_server:Health").then_some(*id))
        })
        .expect("the Health marker");
    assert!(
        r.merged
            .all_edges()
            .any(|e| e.category == edge_category::CONTAINS && e.to == marker),
        "CONTAINED by its file MODULE"
    );
}
