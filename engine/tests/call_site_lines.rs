//! LC.3b: a CALLS edge into a FUNCTION / METHOD, and an IMPORTS edge, carry
//! the exact line of the call / import that asserted them (EVIDENCE basis
//! `site`), not the enclosing declaration's line.
//!
//! One small two-file program per language with CALLS support. Each caller
//! makes its calls below its own declaration line, one into the same file and
//! one into the other file. The check is generic: the source line the
//! evidence names must contain the callee's simple name (an import's line:
//! its target's name or an import keyword), so no row number is hard-coded.
//! CALLS into ENDPOINT and the other call-site-shaped kinds
//! ([`evidence::SITE_KINDS`]) are out of scope: LC.3a locates those at the
//! `to` node, which is the call site.

use std::collections::HashMap;
use std::path::Path;

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Edge, NodeId, NodeKindId};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

/// (language, [(repo-relative path, source)]).
type Program = (&'static str, &'static [(&'static str, &'static str)]);

const PROGRAMS: &[Program] = &[
    (
        "python",
        &[
            ("a.py", "def bar():\n    return 1\n"),
            (
                "b.py",
                "from a import bar\n\n\ndef local():\n    return 2\n\n\n\
                 def baz():\n    x = 1\n    y = local()\n    return bar() + x + y\n",
            ),
        ],
    ),
    (
        "go",
        &[
            ("go.mod", "module example.com/app\n\ngo 1.21\n"),
            (
                "util/util.go",
                "package util\n\nfunc Helper() int {\n\treturn 1\n}\n",
            ),
            (
                "main.go",
                "package main\n\nimport \"example.com/app/util\"\n\n\
                 func local() int {\n\treturn 2\n}\n\n\
                 func run() int {\n\tx := local()\n\treturn util.Helper() + x\n}\n",
            ),
        ],
    ),
    (
        "typescript",
        &[
            ("src/a.ts", "export function bar(): number {\n  return 1;\n}\n"),
            (
                "src/b.ts",
                "import { bar } from \"./a\";\n\nfunction local(): number {\n  return 2;\n}\n\n\
                 export function baz(): number {\n  const x = local();\n  return bar() + x;\n}\n",
            ),
        ],
    ),
    (
        "rust",
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            ("src/a.rs", "pub fn bar() -> i32 {\n    1\n}\n"),
            (
                "src/lib.rs",
                "mod a;\n\nuse crate::a::bar;\n\nfn local() -> i32 {\n    2\n}\n\n\
                 pub fn baz() -> i32 {\n    let x = local();\n    bar() + x\n}\n",
            ),
        ],
    ),
    (
        "java",
        &[
            (
                "src/com/ex/util/Helper.java",
                "package com.ex.util;\n\npublic class Helper {\n    public static int bar() {\n        \
                 return 1;\n    }\n}\n",
            ),
            (
                "src/com/ex/App.java",
                "package com.ex;\n\nimport com.ex.util.Helper;\n\npublic class App {\n    \
                 int local() {\n        return 2;\n    }\n\n    int baz() {\n        \
                 int x = local();\n        return Helper.bar() + x;\n    }\n}\n",
            ),
        ],
    ),
    (
        "csharp",
        &[
            (
                "Util/Helper.cs",
                "namespace Ex.Util\n{\n    public class Helper\n    {\n        \
                 public int Bar()\n        {\n            return 1;\n        }\n    }\n}\n",
            ),
            (
                "App.cs",
                "using Ex.Util;\n\nnamespace Ex\n{\n    public class App\n    {\n        \
                 private readonly Helper _helper;\n\n        \
                 public App(Helper helper)\n        {\n            _helper = helper;\n        }\n\n        \
                 int Local()\n        {\n            return 2;\n        }\n\n        \
                 int Baz()\n        {\n            int x = Local();\n            \
                 return _helper.Bar() + x;\n        }\n    }\n}\n",
            ),
        ],
    ),
    (
        "ruby",
        &[
            ("user_repo.rb", "class UserRepo\n  def find(id)\n    id\n  end\nend\n"),
            (
                "user_service.rb",
                "require_relative \"user_repo\"\n\ndef normalize(id)\n  id\nend\n\n\
                 class UserService\n  def initialize\n    @repo = UserRepo.new\n  end\n\n  \
                 def get(id)\n    x = normalize(id)\n    @repo.find(x)\n  end\nend\n",
            ),
        ],
    ),
    (
        "php",
        &[
            (
                "src/Greeter.php",
                "<?php\nnamespace App\\Services;\n\nclass Greeter\n{\n    \
                 public function greet(): string\n    {\n        return \"hi\";\n    }\n}\n",
            ),
            (
                "src/HomeController.php",
                "<?php\nnamespace App\\Http;\n\nuse App\\Services\\Greeter;\n\n\
                 class HomeController\n{\n    public function show()\n    {\n        \
                 return $this->index(new Greeter());\n    }\n\n    \
                 public function index(Greeter $greeter)\n    {\n        \
                 return $greeter->greet();\n    }\n}\n",
            ),
        ],
    ),
    (
        "swift",
        &[
            (
                "Sources/Shop/Widget+Extras.swift",
                "import Foundation\n\nextension Widget {\n    func more() -> Int {\n        \
                 return 3\n    }\n}\n",
            ),
            (
                "Sources/Shop/Widget.swift",
                "import Foundation\n\nclass Widget {\n    func helper() -> Int {\n        \
                 return 1\n    }\n\n    func run() -> Int {\n        let x = self.helper()\n        \
                 return self.more() + x\n    }\n}\n",
            ),
        ],
    ),
    (
        "c_cpp",
        &[
            (
                "mathutil.h",
                "#pragma once\n\nstatic inline int square(int x) {\n    return x * x;\n}\n",
            ),
            (
                "mathutil.cpp",
                "#include \"mathutil.h\"\n\nstatic int local(int x) {\n    return x + 1;\n}\n\n\
                 int cube(int x) {\n    int y = local(x);\n    return square(y) * y;\n}\n",
            ),
        ],
    ),
    (
        "scala",
        &[
            (
                "util.scala",
                "package myapp.util\n\nobject Greeter {\n  def greet(name: String): String =\n    \
                 \"hi \" + name\n}\n",
            ),
            (
                "main.scala",
                "package myapp.main\n\nimport myapp.util.Greeter\n\nobject Main {\n  \
                 def local(): String = \"x\"\n\n  def run(): String = {\n    val x = local()\n    \
                 Greeter.greet(x)\n  }\n}\n",
            ),
        ],
    ),
    (
        "clojure",
        &[
            ("util.clj", "(ns app.util)\n\n(defn process [x]\n  (inc x))\n"),
            (
                "core.clj",
                "(ns app.core\n  (:require [app.util :as util]))\n\n(defn local [x]\n  (* x 2))\n\n\
                 (defn run [x]\n  (let [y (local x)]\n    (util/process y)))\n",
            ),
        ],
    ),
    (
        "dart",
        &[
            (
                "lib/user_repo.dart",
                "class UserRepo {\n  String find(int id) {\n    return '$id';\n  }\n}\n",
            ),
            (
                "lib/user_service.dart",
                "import 'user_repo.dart';\n\nint local(int id) {\n  return id;\n}\n\n\
                 class UserService {\n  final UserRepo repo;\n\n  UserService(this.repo);\n\n  \
                 String get(int id) {\n    final x = local(id);\n    return repo.find(x);\n  }\n}\n",
            ),
        ],
    ),
    (
        "elixir",
        &[
            (
                "lib/my_app/accounts.ex",
                "defmodule MyApp.Accounts do\n  def get_user(id) do\n    id\n  end\nend\n",
            ),
            (
                "lib/my_app_web/user_controller.ex",
                "defmodule MyAppWeb.UserController do\n  alias MyApp.Accounts\n\n  \
                 def show(conn, id) do\n    x = local(id)\n    Accounts.get_user(x)\n  end\n\n  \
                 def local(id) do\n    id\n  end\nend\n",
            ),
        ],
    ),
    (
        "solidity",
        &[
            (
                "MathLib.sol",
                "// SPDX-License-Identifier: MIT\npragma solidity ^0.8.0;\n\nlibrary MathLib {\n    \
                 function twice(uint256 a) internal pure returns (uint256) {\n        \
                 return a * 2;\n    }\n}\n",
            ),
            (
                "Bank.sol",
                "// SPDX-License-Identifier: MIT\npragma solidity ^0.8.0;\n\nimport \"./MathLib.sol\";\n\n\
                 contract Bank {\n    uint256 total;\n\n    function deposit(uint256 a) public {\n        \
                 _credit(a);\n        total = MathLib.twice(total);\n    }\n\n    \
                 function _credit(uint256 a) internal {\n        total += a;\n    }\n}\n",
            ),
        ],
    ),
    (
        "kotlin",
        &[
            (
                "util.kt",
                "package com.acme.util\n\nfun helper(): Int {\n    return 1\n}\n",
            ),
            (
                "main.kt",
                "package com.acme.main\n\nimport com.acme.util.helper\n\nfun local(): Int {\n    \
                 return 2\n}\n\nfun run(): Int {\n    val x = local()\n    return helper() + x\n}\n",
            ),
        ],
    ),
];

/// Languages whose graph binds no call across files today, with the reason.
/// Their program still makes the cross-file call; the row goes once it binds.
const NO_CROSS_FILE: &[(&str, &str)] = &[(
    "solidity",
    "`import \"./X.sol\"` binds no name and the parser records no field types, \
     so `MathLib.twice()` finds no base",
)];

/// Programs whose references (resolved from an `UnresolvedRef`) name their
/// target on their own line: cross-file heritage, a route handler, a
/// client-router route and link, constructor injection. A decorator-registered
/// handler (`@scheduled_job` above its `def`) is not here: its site is the
/// decorator, one line above the name.
const REF_PROGRAMS: &[Program] = &[
    (
        "python",
        &[
            ("animal.py", "class Animal:\n    pass\n"),
            (
                "dog.py",
                "from animal import Animal\n\n\nclass Dog(\n    Animal,\n):\n    pass\n",
            ),
        ],
    ),
    (
        "go",
        &[
            (
                "main.go",
                "package main\n\nimport (\n\t\"net/http\"\n\n\t\"github.com/go-chi/chi/v5\"\n)\n\n\
                 func listUsers(w http.ResponseWriter, r *http.Request) {}\n\n\
                 func main() {\n\tr := chi.NewRouter()\n\tr.Get(\"/users\", listUsers)\n\
                 \thttp.ListenAndServe(\":8080\", r)\n}\n",
            ),
        ],
    ),
    (
        "typescript",
        &[
            (
                "server/handlers.ts",
                "export function listUsers(req: any, res: any) {\n  res.json([]);\n}\n",
            ),
            (
                "server/app.ts",
                "import express from \"express\";\nimport { listUsers } from \"./handlers\";\n\n\
                 const app = express();\n\napp.get(\"/users\", listUsers);\n",
            ),
        ],
    ),
    (
        "react",
        &[
            (
                "src/pages.tsx",
                "export function Dashboard() {\n  return <div>d</div>;\n}\n\
                 export function Settings() {\n  return <div>s</div>;\n}\n",
            ),
            (
                "src/App.tsx",
                "import { createBrowserRouter, Link } from 'react-router-dom';\n\
                 import { Dashboard, Settings } from './pages';\n\n\
                 export const router = createBrowserRouter([\n\
                 \x20 { path: '/dashboard', element: <Dashboard /> },\n\
                 \x20 { path: '/settings', element: <Settings /> },\n]);\n\n\
                 export function Nav() {\n  return (\n    <div>\n\
                 \x20     <Link to=\"/dashboard\">D</Link>\n    </div>\n  );\n}\n",
            ),
        ],
    ),
    (
        "java",
        &[
            (
                "src/com/ex/Base.java",
                "package com.ex;\n\npublic class Base {\n    public int id() {\n        return 1;\n    }\n}\n",
            ),
            (
                "src/com/ex/Repo.java",
                "package com.ex;\n\npublic class Repo {\n}\n",
            ),
            (
                "src/com/ex/Child.java",
                "package com.ex;\n\nimport org.springframework.stereotype.Service;\n\n@Service\n\
                 public class Child\n        extends Base {\n    private final Repo repo;\n\n    \
                 public Child(\n            Repo repo) {\n        this.repo = repo;\n    }\n}\n",
            ),
        ],
    ),
];

/// Every edge bound from an `UnresolvedRef` (`graph:refs`, `graph:nav`)
/// carries the reference's own line: the source line names the target (a
/// page's name is its path).
#[test]
fn every_ref_edge_points_at_its_reference() {
    let mut failures: Vec<String> = Vec::new();
    let mut report: Vec<String> = Vec::new();
    for (lang, files) in REF_PROGRAMS {
        let (_tmp, repo, m) = build(files);
        let nodes = node_index(&m);
        let mut refs = 0usize;
        for e in m.all_edges() {
            let Some(ev) = Evidence::of(e) else { continue };
            if !matches!(ev.emitter.as_str(), "graph:refs" | "graph:nav") {
                continue;
            }
            refs += 1;
            let name = nodes.get(&e.to).map(|n| n.1.as_str()).unwrap_or("?");
            let from_name = nodes.get(&e.from).map(|n| n.1.as_str()).unwrap_or("?");
            let checked = site_of(e).and_then(|(file, line)| {
                let text = source_line(&repo, &file, line)?;
                if text.contains(name) {
                    Ok(())
                } else {
                    Err(format!("{file}:{line} `{}` does not name `{name}`", text.trim()))
                }
            });
            if let Err(why) = checked {
                failures.push(format!(
                    "{lang}: {} {from_name} -> {name}: {why}",
                    glia_code_domain::edge_category::name(e.category)
                ));
            }
        }
        report.push(format!("{lang}: refs={refs}"));
        if refs == 0 {
            failures.push(format!("{lang}: the program must resolve a reference"));
        }
    }
    eprintln!("[ref-site-lines]\n  {}", report.join("\n  "));
    assert!(failures.is_empty(), "{} failures:\n  {}", failures.len(), failures.join("\n  "));
}

/// Keywords an import statement's line may carry instead of its target's
/// name (`#include`, Elixir `alias`, C# `using`, Ruby `require_relative`).
const IMPORT_WORDS: &[&str] = &["import", "require", "use", "include", "using", "alias"];

/// One language's tallies, for the report line.
#[derive(Default)]
struct Tally {
    calls: usize,
    same_file: usize,
    cross_file: usize,
    imports: usize,
}

fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf, MergedGraph) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    for (rel, src) in files {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, src).unwrap();
    }
    let merged = generate_one(repo.to_str().unwrap()).unwrap().merged;
    (tmp, repo, merged)
}

/// id -> (kind, simple name, POSITION file) over every graph.
fn node_index(m: &MergedGraph) -> HashMap<NodeId, (NodeKindId, String, Option<String>)> {
    let mut out = HashMap::new();
    for g in &m.graphs {
        for n in &g.nodes {
            let Some(kind) = g.nav.kind_by_id.get(&n.id).copied() else { continue };
            let name = g.nav.name_by_id.get(&n.id).cloned().unwrap_or_default();
            let file = glia_code_domain::evidence::locate(&n.cells).map(|(f, _)| f);
            out.entry(n.id).or_insert((kind, name, file));
        }
    }
    out
}

/// The 0-based `line` of `file` under `repo`, or a reason it is missing.
fn source_line(repo: &Path, file: &str, line: u32) -> Result<String, String> {
    let text = std::fs::read_to_string(repo.join(file)).map_err(|e| format!("read {file}: {e}"))?;
    text.lines()
        .nth(line as usize)
        .map(str::to_string)
        .ok_or_else(|| format!("{file} has no line {line}"))
}

/// The site `(file, line)` of `e`'s evidence, or why it is not a site.
fn site_of(e: &Edge) -> Result<(String, u32), String> {
    let ev = Evidence::of(e).ok_or("no EVIDENCE cell")?;
    if ev.basis != Basis::Site {
        return Err(format!("basis {:?}, not site: {ev:?}", ev.basis));
    }
    match (ev.file, ev.line) {
        (Some(f), Some(l)) => Ok((f, l)),
        _ => Err("site evidence without a file and line".to_string()),
    }
}

#[test]
fn every_call_edge_points_at_its_call_expression() {
    let mut failures: Vec<String> = Vec::new();
    let mut report: Vec<String> = Vec::new();
    for (lang, files) in PROGRAMS {
        let (_tmp, repo, m) = build(files);
        let nodes = node_index(&m);
        let mut t = Tally::default();
        for e in m.all_edges() {
            let to = nodes.get(&e.to);
            let to_name = to.map(|n| n.1.as_str()).unwrap_or("?");
            let from_name = nodes.get(&e.from).map(|n| n.1.as_str()).unwrap_or("?");
            if e.category == edge_category::CALLS {
                let Some((kind, name, to_file)) = to else { continue };
                if *kind != node_kind::FUNCTION && *kind != node_kind::METHOD {
                    continue;
                }
                t.calls += 1;
                let from_file = nodes.get(&e.from).and_then(|n| n.2.clone());
                if from_file.is_some() && from_file == *to_file {
                    t.same_file += 1;
                } else {
                    t.cross_file += 1;
                }
                let checked = site_of(e).and_then(|(file, line)| {
                    let text = source_line(&repo, &file, line)?;
                    if text.contains(name.as_str()) {
                        Ok(())
                    } else {
                        Err(format!("{file}:{line} `{}` does not name `{name}`", text.trim()))
                    }
                });
                if let Err(why) = checked {
                    failures.push(format!("{lang}: CALLS {from_name} -> {to_name}: {why}"));
                }
            } else if e.category == edge_category::IMPORTS {
                t.imports += 1;
                let checked = site_of(e).and_then(|(file, line)| {
                    let text = source_line(&repo, &file, line)?;
                    let named = !to_name.is_empty() && text.contains(to_name);
                    if named || IMPORT_WORDS.iter().any(|w| text.contains(w)) {
                        Ok(())
                    } else {
                        Err(format!("{file}:{line} `{}` is not an import of `{to_name}`", text.trim()))
                    }
                });
                if let Err(why) = checked {
                    failures.push(format!("{lang}: IMPORTS {from_name} -> {to_name}: {why}"));
                }
            }
        }
        report.push(format!(
            "{lang}: calls={} (same_file={} cross_file={}) imports={}",
            t.calls, t.same_file, t.cross_file, t.imports
        ));
        let no_cross = NO_CROSS_FILE.iter().find(|(l, _)| l == lang);
        if t.same_file == 0 || (t.cross_file == 0 && no_cross.is_none()) {
            failures.push(format!(
                "{lang}: the program must resolve a same-file and a cross-file FUNCTION/METHOD \
                 call (same_file={} cross_file={})",
                t.same_file, t.cross_file
            ));
        }
        if let Some((_, why)) = no_cross
            && t.cross_file > 0
        {
            failures.push(format!(
                "{lang}: a cross-file call now resolves; drop its NO_CROSS_FILE row ({why})"
            ));
        }
    }
    eprintln!("[call-site-lines]\n  {}", report.join("\n  "));
    assert!(failures.is_empty(), "{} failures:\n  {}", failures.len(), failures.join("\n  "));
}

/// CA.1: a call inside a Go func literal (`once.Do(func() { local() })`) is a
/// CALLS edge of the enclosing function whose site evidence names the
/// closure's row, not the enclosing declaration's.
#[test]
fn go_closure_call_carries_its_site_line() {
    let (_tmp, repo, m) = build(&[
        ("go.mod", "module example.com/app\n\ngo 1.21\n"),
        (
            "main.go",
            "package main\n\nimport \"sync\"\n\n\
             func local() int {\n\treturn 2\n}\n\n\
             func run() {\n\tvar once sync.Once\n\tonce.Do(func() {\n\t\tlocal()\n\t})\n}\n",
        ),
    ]);
    let nodes = node_index(&m);
    let name = |id: &NodeId| nodes.get(id).map(|n| n.1.clone()).unwrap_or_default();
    let calls: Vec<&Edge> = m
        .all_edges()
        .filter(|e| e.category == edge_category::CALLS && name(&e.from) == "run")
        .collect();
    let shown: Vec<(String, String)> = calls.iter().map(|e| (name(&e.from), name(&e.to))).collect();
    assert_eq!(
        shown,
        vec![("run".to_string(), "local".to_string())],
        "CALLS out of run"
    );
    let (file, line) = site_of(calls[0]).unwrap();
    let text = source_line(&repo, &file, line).unwrap();
    // Exact: `func local() int {` also names `local()`.
    assert_eq!(
        text.trim(),
        "local()",
        "{file}:{line} is not the closure's call row"
    );
}
