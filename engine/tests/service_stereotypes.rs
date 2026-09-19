//! LA.21a — service stereotypes for C# / PHP / Ruby / Scala / Elixir / Dart
//! through a real engine build.
//!
//! `services.rs` mints a SERVICE overlay over each classified declaration and
//! LB.3's fold merges it in: the declaration keeps its kind and NodeId and
//! carries ONE ROLE cell naming SERVICE, and no SERVICE twin survives. An
//! Elixir `defmodule` is a PACKAGE, so its overlay folds into the PACKAGE.
//! Declarations are matched by (file, kind, qname leaf), not by full qname,
//! so a parser's qname-shape change does not move this test.

use std::fs;

use glia_code_domain::{cell_type, node_kind};
use glia_core::NodeKindId;
use glia_engine::{generate_one, locate_node};
use glia_graph::MergedGraph;
use glia_graph::roles::roles_in;

const FILES: &[(&str, &str)] = &[
    (
        "UsersController.cs",
        "using Microsoft.AspNetCore.Mvc;\n\nnamespace Shop.Api\n{\n    [ApiController]\n    \
         [Route(\"api/[controller]\")]\n    public class UsersController : ControllerBase\n    {\n        \
         [HttpGet(\"{id}\")]\n        public string Get(int id) { return \"u\"; }\n    }\n\n    \
         public class Foo : Bar\n    {\n        public void Run() {}\n    }\n}\n",
    ),
    (
        "Billing.cs",
        "namespace Shop.Billing\n{\n    public interface IBillingService { void Charge(); }\n\n    \
         public class BillingService : IBillingService\n    {\n        public void Charge() {}\n    }\n\n    \
         public class Worker : BackgroundService\n    {\n        protected override Task \
         ExecuteAsync(CancellationToken t) { return Task.CompletedTask; }\n    }\n}\n",
    ),
    (
        "UserController.php",
        "<?php\nnamespace App\\Http\\Controllers;\n\nclass UserController extends Controller\n{\n    \
         public function show($id) { return $id; }\n}\n",
    ),
    (
        "InvoiceService.php",
        "<?php\nnamespace App\\Services;\n\nclass InvoiceService\n{\n    public function send() {}\n}\n",
    ),
    (
        "users_controller.rb",
        "class UsersController < ApplicationController\n  def show\n  end\nend\n",
    ),
    (
        "signup_service.rb",
        "class SignupService\n  def call\n  end\nend\n",
    ),
    (
        "UserService.scala",
        "package services\n\nimport javax.inject._\n\n@Singleton\nclass UserService @Inject()() {\n  \
         def find(id: Long): Int = 1\n}\n",
    ),
    (
        "worker.ex",
        "defmodule MyApp.Cache do\n  use GenServer\n\n  def start_link(opts), do: \
         GenServer.start_link(__MODULE__, opts)\n  def init(state), do: {:ok, state}\nend\n\n\
         defmodule MyAppWeb.UserController do\n  use MyAppWeb, :controller\n\n  \
         def show(conn, _params), do: conn\nend\n",
    ),
    (
        "auth_service.dart",
        "import 'package:injectable/injectable.dart';\n\n@lazySingleton\nclass AuthService {\n  \
         Future<void> login() async {}\n}\n\nclass CartService extends ChangeNotifier {\n  \
         void add() {}\n}\n",
    ),
];

/// (file, declaration kind, qname leaf, carries the SERVICE role).
const ROWS: &[(&str, NodeKindId, &str, bool)] = &[
    (
        "UsersController.cs",
        node_kind::CLASS,
        "UsersController",
        true,
    ),
    ("Billing.cs", node_kind::CLASS, "BillingService", true),
    ("Billing.cs", node_kind::CLASS, "Worker", true),
    (
        "UserController.php",
        node_kind::CLASS,
        "UserController",
        true,
    ),
    (
        "InvoiceService.php",
        node_kind::CLASS,
        "InvoiceService",
        true,
    ),
    (
        "users_controller.rb",
        node_kind::CLASS,
        "UsersController",
        true,
    ),
    ("signup_service.rb", node_kind::CLASS, "SignupService", true),
    ("UserService.scala", node_kind::CLASS, "UserService", true),
    ("worker.ex", node_kind::PACKAGE, "MyApp.Cache", true),
    (
        "worker.ex",
        node_kind::PACKAGE,
        "MyAppWeb.UserController",
        true,
    ),
    ("auth_service.dart", node_kind::CLASS, "AuthService", true),
    // Negatives: no controller / hosted / `I`+name base; no DI annotation.
    ("UsersController.cs", node_kind::CLASS, "Foo", false),
    ("Billing.cs", node_kind::INTERFACE, "IBillingService", false),
    ("auth_service.dart", node_kind::CLASS, "CartService", false),
];

fn build() -> MergedGraph {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, body) in FILES {
        fs::write(dir.path().join(name), body).expect("write fixture file");
    }
    let repo = dir.path().to_str().expect("utf-8 tempdir");
    generate_one(repo).expect("generate_one").merged
}

#[test]
fn six_languages_carry_the_service_role_on_their_declarations() {
    let merged = build();

    let twins: Vec<String> = merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter_map(|n| {
                (g.nav.kind_by_id.get(&n.id) == Some(&node_kind::SERVICE))
                    .then(|| g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default())
            })
        })
        .collect();
    assert!(
        twins.is_empty(),
        "SERVICE twins survived the fold: {twins:?}"
    );

    let mut failures: Vec<String> = Vec::new();
    for &(file, kind, leaf, service) in ROWS {
        let mut matched = Vec::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                let k = g.nav.kind_by_id.get(&n.id).copied();
                let q = g.nav.qname_by_id.get(&n.id).map_or("", String::as_str);
                if k != Some(kind) || q.rsplit("::").next() != Some(leaf) {
                    continue;
                }
                if locate_node(&merged, n.id).file.as_deref() != Some(file) {
                    continue;
                }
                matched.push((
                    roles_in(k, &n.cells),
                    n.cells.iter().filter(|c| c.kind == cell_type::ROLE).count(),
                ));
            }
        }
        let row = format!("{file} {} {leaf}", node_kind::name(kind));
        match matched.as_slice() {
            [(roles, role_cells)] => {
                let has = roles.contains(&node_kind::SERVICE);
                if has != service {
                    failures.push(format!("{row}: SERVICE role {has}, expected {service}"));
                } else if service && *role_cells != 1 {
                    failures.push(format!("{row}: {role_cells} ROLE cells, expected ONE"));
                }
            }
            other => failures.push(format!(
                "{row}: {} matching declarations, expected 1",
                other.len()
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "{} rows failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
