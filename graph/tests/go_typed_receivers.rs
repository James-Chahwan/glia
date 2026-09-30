//! CA.2b: Go typed receivers. A method call whose receiver is a call chain
//! (`services.UserRepository().FindByID()`), a local or parameter, a
//! package-level var or a struct-field chain (`s.deps.repo.Save()`) binds the
//! method on that receiver's type, which the Go call hook
//! (`build::GoPackages`) reads off CA.2a's recorded facts: `return_types`,
//! `local_types` (fn scopes and, for package vars, file MODULE scopes) and
//! `field_types`. Each file is parsed the way the engine does (module qname =
//! path minus `.go`, `/` -> `::`), so the parser's facts are exercised end to
//! end, as in `go_package_dir.rs`.

use std::path::PathBuf;

use glia_code_domain::evidence::Evidence;
use glia_core::{Edge, NodeId, RepoId};
use glia_graph::{RepoGraph, build_go};
use glia_parser_go::{CallQualifier, FileParse, GRAPH_TYPE, edge_category, node_kind, parse_file};

fn repo() -> RepoId {
    RepoId::from_canonical("test://go_typed_receivers")
}

/// The engine's `path_to_qname`: drop `.go`, `/` -> `::`.
fn parse_in(prefix: &str, rel: &str, src: &str) -> FileParse {
    let qname = rel.strip_suffix(".go").unwrap_or(rel).replace('/', "::");
    parse_file(src, rel, &qname, prefix, repo()).unwrap()
}

fn parse(rel: &str, src: &str) -> FileParse {
    parse_in("example.com/app", rel, src)
}

fn func(qname: &str) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, qname)
}
fn method(qname: &str) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, qname)
}

/// The CALLS edges `from -> to` (a builder never pushes one twice).
fn calls(g: &RepoGraph, from: NodeId, to: NodeId) -> Vec<&Edge> {
    g.edges
        .iter()
        .filter(|e| e.from == from && e.to == to && e.category == edge_category::CALLS)
        .collect()
}

/// The one CALLS edge `from -> to`, as `(emitter, rule, 0-based line)`.
fn call_evidence(g: &RepoGraph, from: NodeId, to: NodeId) -> (String, Option<String>, Option<u32>) {
    let hits = calls(g, from, to);
    assert_eq!(hits.len(), 1, "one CALLS edge {from:?} -> {to:?}: {hits:?}");
    let ev = Evidence::of(hits[0]).expect("EVIDENCE cell");
    (ev.emitter, ev.rule, ev.line)
}

fn go_packages(rule: &str, line: u32) -> (String, Option<String>, Option<u32>) {
    (
        "graph:go_packages".to_string(),
        Some(rule.to_string()),
        Some(line),
    )
}

/// True when a site calling `name` from `from` stayed unresolved.
fn unresolved(g: &RepoGraph, from: NodeId, name: &str) -> bool {
    g.unresolved_calls.iter().any(|s| {
        s.from == from
            && match &s.qualifier {
                CallQualifier::Attribute { name: n, .. }
                | CallQualifier::ComplexReceiver { name: n, .. } => n == name,
                _ => false,
            }
    })
}

fn fixture_parses() -> Vec<FileParse> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("bench/substrate-gap/fixtures/go-return-type-receivers");
    [
        "handlers/users.go",
        "repositories/order_repository.go",
        "repositories/user_repository.go",
        "services/provider.go",
    ]
    .iter()
    .map(|rel| {
        parse_in(
            "example.com/shop",
            rel,
            &std::fs::read_to_string(root.join(rel)).unwrap(),
        )
    })
    .collect()
}

/// The bench fixture: a chained receiver (the inner call's return type), a
/// local bound to a call, a package-qualified parameter and a package var
/// initialised by a call each bind the method on `UserRepository`, never on
/// `OrderRepository`, which repeats every method name. The FUNCTION
/// `services::provider::UserRepository` shares its name with the STRUCT.
#[test]
fn go_typed_receiver_fixture() {
    let g = build_go(repo(), fixture_parses()).unwrap();
    let get = func("handlers::users::GetUser");
    let save = func("handlers::users::SaveUser");
    let count = func("handlers::users::CountUsers");
    let default = func("handlers::users::DefaultUser");
    let user = |m: &str| {
        method(&format!(
            "repositories::user_repository::UserRepository::{m}"
        ))
    };
    let order = |m: &str| {
        method(&format!(
            "repositories::order_repository::OrderRepository::{m}"
        ))
    };

    // CONTROL: the inner call binds today (the generic attribute branch,
    // through the import of `services`).
    assert_eq!(
        call_evidence(&g, get, func("services::provider::UserRepository")),
        (
            "graph:calls".to_string(),
            Some("attribute".to_string()),
            Some(10)
        )
    );
    assert_eq!(
        call_evidence(&g, get, user("FindByID")),
        go_packages("receiver_return", 10)
    );
    assert_eq!(
        call_evidence(&g, save, user("Save")),
        go_packages("receiver_local", 15)
    );
    assert_eq!(
        call_evidence(&g, count, user("Count")),
        go_packages("receiver_local", 19)
    );
    assert_eq!(
        call_evidence(&g, default, user("FindByID")),
        go_packages("receiver_package_var", 23)
    );
    for (from, m) in [
        (get, "FindByID"),
        (save, "Save"),
        (count, "Count"),
        (default, "FindByID"),
    ] {
        assert!(
            calls(&g, from, order(m)).is_empty(),
            "no CALLS to OrderRepository::{m}"
        );
        assert!(
            !unresolved(&g, from, m),
            "the {m} site left unresolved_calls"
        );
    }
}

/// `s.deps.repo.Save()`: the receiver's struct, its field `deps` (a
/// same-package type) and that type's field `repo` (a type of an imported
/// package, recorded by its bare name) type the receiver; the other `Save`
/// never binds.
#[test]
fn go_typed_receiver_field_chain() {
    let parses = vec![
        parse(
            "svc/svc.go",
            "package svc\n\ntype Svc struct {\n\tdeps *Deps\n}\n\nfunc (s *Svc) Run() {\n\ts.deps.repo.Save()\n}\n",
        ),
        parse(
            "svc/deps.go",
            "package svc\n\nimport \"example.com/app/store\"\n\ntype Deps struct {\n\trepo *store.Repo\n}\n",
        ),
        parse(
            "store/store.go",
            "package store\n\ntype Repo struct{}\n\nfunc (r *Repo) Save() {}\n",
        ),
        parse(
            "other/other.go",
            "package other\n\ntype Cache struct{}\n\nfunc (c *Cache) Save() {}\n",
        ),
    ];
    let g = build_go(repo(), parses).unwrap();
    let run = method("svc::svc::Svc::Run");
    assert_eq!(
        call_evidence(&g, run, method("store::store::Repo::Save")),
        go_packages("receiver_field_chain", 7)
    );
    assert!(calls(&g, run, method("other::other::Cache::Save")).is_empty());
}

/// A FUNCTION and a STRUCT share the name `UserRepository` (quokka's
/// repository_provider.go beside user_repository.go). The generic pass's
/// type lookup has no kind filter, so the field's bare type `UserRepository`
/// is ambiguous there; the hook's type lookups accept STRUCT / INTERFACE
/// only, so the STRUCT answers.
#[test]
fn go_typed_receiver_kind_filter() {
    let parses = vec![
        parse(
            "repositories/user_repository.go",
            "package repositories\n\ntype UserRepository struct{}\n\nfunc (r *UserRepository) Save() {}\n",
        ),
        parse(
            "services/provider.go",
            "package services\n\nimport \"example.com/app/repositories\"\n\nfunc UserRepository() *repositories.UserRepository {\n\treturn nil\n}\n",
        ),
        parse(
            "handlers/h.go",
            "package handlers\n\nimport \"example.com/app/repositories\"\n\ntype Handler struct {\n\trepo *repositories.UserRepository\n}\n\nfunc (h *Handler) Run() {\n\th.repo.Save()\n}\n",
        ),
    ];
    let g = build_go(repo(), parses).unwrap();
    let run = method("handlers::h::Handler::Run");
    assert_eq!(
        call_evidence(
            &g,
            run,
            method("repositories::user_repository::UserRepository::Save")
        ),
        go_packages("receiver_field_chain", 9)
    );
}

/// `repositories.UserRepository(x).Save()`: a conversion to an imported
/// package's type types the receiver as that type.
#[test]
fn go_typed_receiver_conversion() {
    let parses = vec![
        parse(
            "repositories/user_repository.go",
            "package repositories\n\ntype UserRepository struct{}\n\nfunc (r UserRepository) Save() {}\n",
        ),
        parse(
            "repositories/order_repository.go",
            "package repositories\n\ntype OrderRepository struct{}\n\nfunc (r OrderRepository) Save() {}\n",
        ),
        parse(
            "handlers/h.go",
            "package handlers\n\nimport \"example.com/app/repositories\"\n\nfunc Run(x struct{}) {\n\trepositories.UserRepository(x).Save()\n}\n",
        ),
    ];
    let g = build_go(repo(), parses).unwrap();
    let run = func("handlers::h::Run");
    assert_eq!(
        call_evidence(
            &g,
            run,
            method("repositories::user_repository::UserRepository::Save")
        ),
        go_packages("receiver_return", 5)
    );
    assert!(
        calls(
            &g,
            run,
            method("repositories::order_repository::OrderRepository::Save")
        )
        .is_empty()
    );
}

/// Two STRUCTs `Store` in two other packages and a bare type text `Store`
/// the caller's package does not declare: no lookup is unique, so nothing
/// binds and the site stays unresolved (never first-wins).
#[test]
fn go_typed_receiver_ambiguous() {
    let parses = vec![
        parse(
            "a/store.go",
            "package a\n\ntype Store struct{}\n\nfunc (s *Store) Save() {}\n",
        ),
        parse(
            "b/store.go",
            "package b\n\ntype Store struct{}\n\nfunc (s *Store) Save() {}\n",
        ),
        parse(
            "c/c.go",
            "package c\n\nfunc Use(s *Store) {\n\ts.Save()\n}\n",
        ),
    ];
    let g = build_go(repo(), parses).unwrap();
    let use_ = func("c::c::Use");
    assert!(calls(&g, use_, method("a::store::Store::Save")).is_empty());
    assert!(calls(&g, use_, method("b::store::Store::Save")).is_empty());
    assert!(
        unresolved(&g, use_, "Save"),
        "the site stays in unresolved_calls"
    );
}

/// A same-file `u *User` parameter binds through the generic receiver pass
/// (A6.2a / LA.35a), evidence `graph:calls` rule `receiver_type`: the hook
/// runs only after every generic lookup missed, so it never re-binds a hit.
#[test]
fn go_typed_receiver_keeps_generic_hits() {
    let parses = vec![
        parse(
            "users/user.go",
            "package users\n\ntype User struct{}\n\nfunc (u *User) Save() {}\n\nfunc Do(u *User) {\n\tu.Save()\n}\n",
        ),
        parse(
            "other/other.go",
            "package other\n\ntype User struct{}\n\nfunc (u *User) Save() {}\n",
        ),
    ];
    let g = build_go(repo(), parses).unwrap();
    assert_eq!(
        call_evidence(
            &g,
            func("users::user::Do"),
            method("users::user::User::Save")
        ),
        (
            "graph:calls".to_string(),
            Some("receiver_type".to_string()),
            Some(7)
        )
    );
}

/// quokka's chat_controller.go:82 / :192 shape (the engram-6c finding): a
/// local bound to an imported package's getter, `natsService :=
/// chat.GetGlobalNATSService()`, whose result type is declared in another
/// file of that package, then `natsService.GetRoomMessages(..)`.
#[test]
fn go_typed_receiver_local_from_imported_getter() {
    let parses = vec![
        parse(
            "Services/chat/nats.go",
            "package chat\n\ntype NATSChatService struct{}\n\nfunc (s *NATSChatService) GetRoomMessages(room string, max int) error { return nil }\n",
        ),
        parse(
            "Services/chat/notifications.go",
            "package chat\n\nvar globalNATSService *NATSChatService\n\nfunc GetGlobalNATSService() *NATSChatService {\n\treturn globalNATSService\n}\n",
        ),
        parse(
            "Server/Controllers/chat_controller.go",
            "package controllers\n\nimport (\n\tchat \"example.com/app/Services/chat\"\n)\n\nfunc GetChatPreviewHandler() {\n\tnatsService := chat.GetGlobalNATSService()\n\tif natsService == nil {\n\t\treturn\n\t}\n\t_ = natsService.GetRoomMessages(\"r\", 1)\n}\n",
        ),
    ];
    let g = build_go(repo(), parses).unwrap();
    let handler = func("Server::Controllers::chat_controller::GetChatPreviewHandler");
    assert_eq!(
        call_evidence(
            &g,
            handler,
            func("Services::chat::notifications::GetGlobalNATSService")
        ),
        go_packages("package_import", 7)
    );
    assert_eq!(
        call_evidence(
            &g,
            handler,
            method("Services::chat::nats::NATSChatService::GetRoomMessages")
        ),
        go_packages("receiver_local", 11)
    );
}
