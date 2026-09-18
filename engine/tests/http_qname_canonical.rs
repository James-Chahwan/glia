//! LB.5 acceptance: every ROUTE / ENDPOINT qname carries its path in the one
//! canonical form (`code_domain::endpoint::canonical_http_path`: a single
//! leading `/`, or an exempt `${…}` / `<unresolved>` / empty placeholder).
//!
//! Before LB.5 only the parsers that called `abs_path` produced it. Go's
//! `join_path` kept an unprefixed literal relative (`route:items`), and rust,
//! clojure and the TypeScript client built theirs with a bare `format!`
//! (`GET widgets`, `endpoint:GET:widgets`). The HTTP resolver indexes the
//! legacy `<METHOD> <path>` shape only when the path starts with `/`, so a
//! relative rust / clojure route never paired, and a relative and a slashed
//! client call to one path were two ENDPOINT nodes.
//!
//! One tempdir per language, each with a RELATIVE route literal and a
//! RELATIVE client call. Where a parser drops a relative literal outright
//! (url_to_path-gated clients, dart shelf) that is a recall gap, not a
//! normalisation one: the directory still guards that nothing it does emit is
//! non-canonical.

use repo_graph_code_domain::endpoint::is_canonical_http_path;
use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_engine::generate_one;
use repo_graph_graph::MergedGraph;

/// Write `files` under a fresh tempdir and build it as one repo.
fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let td = tempfile::tempdir().expect("tempdir");
    for (rel, src) in files {
        let path = td.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, src).expect("write source");
    }
    let root = td.path().to_string_lossy().into_owned();
    let merged = generate_one(&root).expect("generate_one").merged;
    (td, merged)
}

/// Every ROUTE and ENDPOINT qname in the build.
fn http_qnames(m: &MergedGraph) -> Vec<String> {
    let mut out: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav.kind_by_id.iter().filter_map(|(id, k)| {
                (*k == node_kind::ROUTE || *k == node_kind::ENDPOINT)
                    .then(|| g.nav.qname_by_id.get(id).cloned())
                    .flatten()
            })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The path part of a ROUTE / ENDPOINT qname, in any of the three shapes.
fn path_part(qname: &str) -> Option<&str> {
    if let Some(rest) = qname.strip_prefix("endpoint:") {
        return rest.split_once(':').map(|(_, p)| p);
    }
    qname
        .strip_prefix("route:")
        .or_else(|| qname.split_once(' ').map(|(_, p)| p))
}

fn has_http_call(m: &MergedGraph, from: &str, to: &str) -> bool {
    let (Some(f), Some(t)) = (m.node_id_by_qname(from), m.node_id_by_qname(to)) else {
        return false;
    };
    m.cross_edges
        .iter()
        .any(|e| e.from == f && e.to == t && e.category == edge_category::HTTP_CALLS)
}

const GO: &[(&str, &str)] = &[(
    "main.go",
    "package main\n\n\
     import (\n\
     \t\"net/http\"\n\n\
     \t\"github.com/gin-gonic/gin\"\n\
     \t\"github.com/labstack/echo/v4\"\n\
     )\n\n\
     func listItems(c *gin.Context) {}\n\
     func listParts(c echo.Context) error { return nil }\n\n\
     func main() {\n\
     \tr := gin.Default()\n\
     \tr.GET(\"items\", listItems)\n\
     \te := echo.New()\n\
     \te.GET(\"parts\", listParts)\n\
     \thttp.Get(\"items\")\n\
     }\n",
)];

const RUST_TS: &[(&str, &str)] = &[
    (
        "server.rs",
        "use axum::{Router, routing::get};\n\n\
         async fn list_widgets() {}\n\n\
         pub fn app() -> Router {\n    Router::new().route(\"widgets\", get(list_widgets))\n}\n",
    ),
    (
        "client.ts",
        "export async function loadWidgets() {\n  return fetch('widgets');\n}\n",
    ),
];

const CLOJURE: &[(&str, &str)] = &[(
    "handler.clj",
    "(ns app.handler\n  (:require [compojure.core :refer [GET POST defroutes]]\n            [clj-http.client :as client]))\n\n\
     (defn list-bolts [] \"ok\")\n\n\
     (defroutes app-routes\n  (GET \"bolts\" [] (list-bolts))\n  (POST \"bolts\" [] (list-bolts)))\n\n\
     (defn fetch-bolts [] (client/get \"bolts\"))\n",
)];

const TS: &[(&str, &str)] = &[
    (
        "api.ts",
        "import axios from 'axios';\n\n\
         export async function loadA() {\n  return fetch('alpha');\n}\n\n\
         export async function loadB() {\n  return axios.get('beta');\n}\n",
    ),
    (
        "user.service.ts",
        "import { Injectable } from '@angular/core';\n\
         import { HttpClient } from '@angular/common/http';\n\n\
         @Injectable({ providedIn: 'root' })\n\
         export class UserService {\n\
         \x20 constructor(private http: HttpClient) {}\n\n\
         \x20 remove() {\n    return this.http.delete('protected/settings/account');\n  }\n\
         }\n",
    ),
];

const PYTHON: &[(&str, &str)] = &[(
    "app.py",
    "import requests\nfrom flask import Flask\n\napp = Flask(__name__)\n\n\n\
     @app.route(\"users\")\ndef users():\n    return {\"users\": []}\n\n\n\
     def fetch():\n    return requests.get(\"users\")\n",
)];

const JAVA: &[(&str, &str)] = &[(
    "UserController.java",
    "package com.example;\n\n\
     import org.springframework.web.bind.annotation.GetMapping;\n\
     import org.springframework.web.bind.annotation.RequestMapping;\n\
     import org.springframework.web.bind.annotation.RestController;\n\
     import org.springframework.web.client.RestTemplate;\n\n\
     @RestController\n@RequestMapping(\"api\")\npublic class UserController {\n\
     \x20   private final RestTemplate rest = new RestTemplate();\n\n\
     \x20   @GetMapping(\"users\")\n    public String list() {\n\
     \x20       return rest.getForObject(\"users\", String.class);\n    }\n}\n",
)];

const PHP: &[(&str, &str)] = &[(
    "web.php",
    "<?php\n\nuse Illuminate\\Support\\Facades\\Route;\n\n\
     Route::get('users', [UserController::class, 'index']);\n",
)];

const RUBY: &[(&str, &str)] = &[(
    "config/routes.rb",
    "Rails.application.routes.draw do\n  get \"users\" => \"users#index\"\nend\n",
)];

const SCALA: &[(&str, &str)] = &[(
    "Routes.scala",
    "package app\n\nimport akka.http.scaladsl.server.Directives._\n\n\
     object Routes {\n  val route = path(\"users\") {\n    get {\n      complete(\"ok\")\n    }\n  }\n}\n",
)];

const SWIFT: &[(&str, &str)] = &[(
    "routes.swift",
    "import Vapor\n\nfunc routes(_ app: Application) throws {\n    app.get(\"users\") { req in \"list\" }\n}\n",
)];

const ELIXIR: &[(&str, &str)] = &[(
    "router.ex",
    "defmodule MyAppWeb.Router do\n  use MyAppWeb, :router\n\n\
     \x20 scope \"api\", MyAppWeb do\n    get \"users\", UserController, :index\n  end\nend\n",
)];

const DART: &[(&str, &str)] = &[(
    "lib/server.dart",
    "import 'package:shelf_router/shelf_router.dart';\nimport 'package:dio/dio.dart';\n\n\
     final app = Router();\n\n\
     void setup(Dio dio) {\n  app.get('users', (req) => 'ok');\n  dio.get('users');\n}\n",
)];

const CSHARP: &[(&str, &str)] = &[(
    "UsersController.cs",
    "using System.Net.Http;\nusing Microsoft.AspNetCore.Mvc;\n\n\
     namespace Shop.Controllers\n{\n    [ApiController]\n    [Route(\"api/users\")]\n\
     \x20   public class UsersController : ControllerBase\n    {\n\
     \x20       private readonly HttpClient _http;\n\n\
     \x20       [HttpGet(\"list\")]\n        public async Task<string> List()\n        {\n\
     \x20           var res = await _http.GetAsync(\"api/users/list\");\n\
     \x20           return \"ok\";\n        }\n    }\n}\n",
)];

/// `(label, sources, qnames that MUST be present)`. The must-list keeps each
/// directory from passing vacuously; an empty list marks a parser that drops
/// the relative literal (a recall gap outside LB.5).
fn cases() -> Vec<(
    &'static str,
    &'static [(&'static str, &'static str)],
    Vec<&'static str>,
)> {
    vec![
        ("go gin + echo", GO, vec!["route:/items", "route:/parts"]),
        (
            "rust axum + ts fetch",
            RUST_TS,
            vec!["GET /widgets", "endpoint:GET:/widgets"],
        ),
        (
            "clojure compojure",
            CLOJURE,
            vec!["GET /bolts", "POST /bolts"],
        ),
        (
            "ts fetch + axios + angular",
            TS,
            vec![
                "endpoint:GET:/alpha",
                "endpoint:GET:/beta",
                "endpoint:DELETE:/protected/settings/account",
            ],
        ),
        ("python flask + requests", PYTHON, vec!["GET /users"]),
        ("java spring + resttemplate", JAVA, vec!["GET /api/users"]),
        ("php laravel", PHP, vec!["GET /users"]),
        ("ruby rails", RUBY, vec!["GET /users"]),
        ("scala akka", SCALA, vec!["ANY /users"]),
        ("swift vapor", SWIFT, vec!["GET /users"]),
        ("elixir phoenix", ELIXIR, vec!["GET /api/users"]),
        ("dart shelf + dio", DART, vec![]),
        (
            "csharp aspnet + httpclient",
            CSHARP,
            vec!["GET /api/users/list"],
        ),
    ]
}

#[test]
fn every_language_emits_canonical_http_qnames() {
    let mut failures: Vec<String> = Vec::new();
    for (label, files, must) in cases() {
        let (_td, m) = build(files);
        let qnames = http_qnames(&m);
        for q in &qnames {
            if path_part(q).is_some_and(|p| !is_canonical_http_path(p)) {
                failures.push(format!("{label}: non-canonical `{q}`"));
            }
        }
        for q in must {
            if !qnames.iter().any(|x| x == q) {
                failures.push(format!("{label}: missing `{q}` (have {qnames:?})"));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// A relative rust route used to be skipped by the route index (it requires
/// a leading `/`), so even a correct client could not reach it.
#[test]
fn relative_rust_route_pairs_with_relative_ts_call() {
    let (_td, m) = build(RUST_TS);
    assert!(
        has_http_call(&m, "endpoint:GET:/widgets", "GET /widgets"),
        "HTTP_CALLS endpoint:GET:/widgets -> GET /widgets missing; qnames {:?}",
        http_qnames(&m)
    );
}

/// quokka-stack's shape: one Angular service calls `protected/x`, another
/// `/protected/x`. One server endpoint must be ONE graph node, carrying both
/// call sites' ENDPOINT_HIT cells.
#[test]
fn merges_relative_and_slashed_calls() {
    let service = |name: &str, path: &str| {
        format!(
            "import {{ HttpClient }} from '@angular/common/http';\n\n\
             export class {name} {{\n  constructor(private http: HttpClient) {{}}\n\n\
             \x20 remove() {{\n    return this.http.delete('{path}');\n  }}\n}}\n"
        )
    };
    let a = service("AccountService", "protected/x");
    let b = service("SettingsService", "/protected/x");
    let (_td, m) = build(&[
        ("account.service.ts", a.as_str()),
        ("settings.service.ts", b.as_str()),
    ]);

    let endpoints: Vec<String> = http_qnames(&m)
        .into_iter()
        .filter(|q| q.starts_with("endpoint:"))
        .collect();
    assert_eq!(endpoints, vec!["endpoint:DELETE:/protected/x".to_string()]);

    let id = m
        .node_id_by_qname("endpoint:DELETE:/protected/x")
        .expect("canonical endpoint");
    let nodes: Vec<_> = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .collect();
    assert_eq!(nodes.len(), 1, "one ENDPOINT node");
    let hits = nodes[0]
        .cells
        .iter()
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
        .count();
    assert_eq!(hits, 2, "both call sites' ENDPOINT_HIT cells stack on it");
}
