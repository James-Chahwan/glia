//! FH.21: the call / ref passes decide each site on a pool and apply the
//! decisions in input order, so a graph is the same whatever pool ran it.
//!
//! Two checks. (1) The graph crate's builders, called directly on parses,
//! give the same nodes, edges and unresolved lists with no pool installed
//! (one chunk, this thread) and under [`glia_graph::with_resolve_pool`]
//! with pools that cut the sites into one-site chunks and run them on eight
//! workers, or on this thread newest chunk first. (2) `generate_one` on a
//! TypeScript + Go + Rust repo writes byte-identical layouts on a one-thread
//! and a sixteen-thread rayon pool (the engine installs its own resolve pool
//! over whichever pool the build runs on).

use std::path::Path;
use std::sync::Arc;

use glia_core::RepoId;
use glia_engine::{generate_one, parse_one, parse_one_with};
use glia_graph::{RepoGraph, ResolvePool, with_resolve_pool};
use glia_store::write_merged_sharded;
use rayon::prelude::*;

const STACK: usize = 16 << 20;

/// A pool that makes every site its own chunk and runs the chunks on a
/// rayon pool of `threads` workers.
struct Workers(rayon::ThreadPool);

impl ResolvePool for Workers {
    fn threads(&self) -> usize {
        self.0.current_num_threads()
    }

    fn chunk_floor(&self) -> usize {
        1
    }

    fn run(&self, n: usize, job: &(dyn Fn(usize) + Sync)) {
        self.0.install(|| (0..n).into_par_iter().for_each(job));
    }
}

/// A pool that runs one-site chunks on the calling thread, last chunk first:
/// any decision that leaked state from one site to the next would move.
struct Backwards;

impl ResolvePool for Backwards {
    fn threads(&self) -> usize {
        8
    }

    fn chunk_floor(&self) -> usize {
        1
    }

    fn run(&self, n: usize, job: &(dyn Fn(usize) + Sync)) {
        for i in (0..n).rev() {
            job(i);
        }
    }
}

fn workers(n: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .stack_size(STACK)
        .build()
        .unwrap()
}

/// Everything the passes write, in order: nodes, edges (cells included),
/// and the sites and refs left unresolved.
fn fingerprint(g: &RepoGraph) -> String {
    format!(
        "{:#?}\n{:#?}\n{:#?}\n{:#?}",
        g.nodes, g.edges, g.unresolved_calls, g.unresolved_refs
    )
}

/// The TypeScript fixture: a class chain (Base <- Svc <- Feature<i>), a
/// re-export barrel, injected and typed receivers, a call-result local, a
/// constructed receiver, `this` / `super` calls the inherited pass binds,
/// and calls that bind nothing.
fn ts_files(features: usize) -> Vec<(String, String)> {
    let mut files = vec![
        (
            "src/base.ts".to_string(),
            "export class Base {\n  greet(): string { return \"hi\"; }\n  shared(): number { return 1; }\n}\n\
             export class Store {\n  reset(): void {}\n}\n\
             export function makeStore(): Store { return new Store(); }\n"
                .to_string(),
        ),
        (
            "src/svc.ts".to_string(),
            "import { Base } from './base';\n\
             export class Svc extends Base {\n  run(): number { this.greet(); return super.shared(); }\n}\n"
                .to_string(),
        ),
        (
            "src/models/user.ts".to_string(),
            "export class User {\n  name(): string { return \"\"; }\n}\n".to_string(),
        ),
        (
            "src/models/index.ts".to_string(),
            "export * from './user';\n".to_string(),
        ),
    ];
    for i in 0..features {
        files.push((
            format!("src/feat{i}.ts"),
            format!(
                "import {{ Svc }} from './svc';\n\
                 import {{ User }} from './models';\n\
                 import {{ makeStore, Store }} from './base';\n\
                 export class Feature{i} extends Svc {{\n\
                 \x20 constructor(private svc: Svc, private user: User) {{ super(); }}\n\
                 \x20 go{i}(): void {{\n\
                 \x20   this.run();\n\
                 \x20   this.greet();\n\
                 \x20   this.svc.run();\n\
                 \x20   this.svc.greet();\n\
                 \x20   this.user.name();\n\
                 \x20   const store = makeStore();\n\
                 \x20   store.reset();\n\
                 \x20   new Store().reset();\n\
                 \x20   helper{i}();\n\
                 \x20   missing{i}();\n\
                 \x20 }}\n\
                 }}\n\
                 function helper{i}(): number {{ return {i}; }}\n"
            ),
        ));
    }
    files
}

fn go_files(services: usize) -> Vec<(String, String)> {
    let mut files = vec![
        (
            "go.mod".to_string(),
            "module example.com/fx\n\ngo 1.22\n".to_string(),
        ),
        (
            "store/store.go".to_string(),
            "package store\n\ntype Store struct{}\n\nfunc (s *Store) Get() int { return 1 }\n\n\
             func New() *Store { return &Store{} }\n"
                .to_string(),
        ),
    ];
    for i in 0..services {
        files.push((
            format!("svc/svc{i}.go"),
            format!(
                "package svc\n\nimport \"example.com/fx/store\"\n\n\
                 type S{i} struct {{\n\tst *store.Store\n}}\n\n\
                 func (s *S{i}) Run() int {{ return s.st.Get() + store.New().Get() + helper{i}() + s.Twice() }}\n\n\
                 func (s *S{i}) Twice() int {{ return 2 }}\n\n\
                 func helper{i}() int {{ return {i} + gone{i}() }}\n"
            ),
        ));
    }
    files
}

fn rust_files(mods: usize) -> Vec<(String, String)> {
    let mut files = vec![
        (
            "Cargo.toml".to_string(),
            "[package]\nname = \"fx\"\nversion = \"0.1.0\"\nedition = \"2021\"\n".to_string(),
        ),
        (
            "src/util.rs".to_string(),
            "pub struct Tally(pub u32);\n\nimpl Tally {\n    pub fn bump(&self) -> u32 { self.0 + 1 }\n}\n\n\
             pub fn base() -> u32 { 7 }\n"
                .to_string(),
        ),
    ];
    let mut lib = String::from("pub mod util;\n");
    for i in 0..mods {
        lib.push_str(&format!("pub mod m{i};\n"));
        files.push((
            format!("src/m{i}.rs"),
            format!(
                "use crate::util::{{base, Tally}};\n\n\
                 pub fn run{i}() -> u32 {{\n    let t = Tally({i});\n    t.bump() + base() + crate::util::base() + local{i}() + absent{i}()\n}}\n\n\
                 fn local{i}() -> u32 {{ {i} }}\n"
            ),
        ));
    }
    files.push(("src/lib.rs".to_string(), lib));
    files
}

fn write_all(dir: &Path, files: &[(String, String)]) {
    for (rel, body) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
}

/// The engine's relative TS specifier rule: the importer's directory plus
/// the specifier, `::`-joined, extension dropped.
fn ts_source(from_module: &str, specifier: &str) -> Option<String> {
    let spec = specifier.trim().trim_matches(|c| c == '"' || c == '\'');
    if !spec.starts_with('.') {
        return None;
    }
    let mut segs: Vec<String> = from_module.split("::").map(String::from).collect();
    segs.pop();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            p => segs.push(p.trim_end_matches(".ts").to_string()),
        }
    }
    Some(segs.join("::"))
}

/// Build one language's graph three ways (no pool, eight workers with
/// one-site chunks, one-site chunks newest first) and require one answer.
fn same_graph_every_way(label: &str, build: impl Fn() -> RepoGraph) {
    let sequential = build();
    assert!(
        !sequential.edges.is_empty(),
        "{label}: the fixture must bind something"
    );
    assert!(
        !sequential.unresolved_calls.is_empty(),
        "{label}: the fixture must leave a call unresolved"
    );
    let want = fingerprint(&sequential);
    let pooled = with_resolve_pool(Arc::new(Workers(workers(8))), &build);
    assert_eq!(
        fingerprint(&pooled),
        want,
        "{label}: eight workers, one-site chunks"
    );
    let backwards = with_resolve_pool(Arc::new(Backwards), &build);
    assert_eq!(
        fingerprint(&backwards),
        want,
        "{label}: one-site chunks run newest first"
    );
}

#[test]
fn typescript_family_graph_is_the_same_pooled() {
    let repo = RepoId(21);
    let files = ts_files(24);
    let build = || {
        let parses = files
            .iter()
            .map(|(path, src)| parse_one(src, path, "typescript", repo).unwrap())
            .collect();
        glia_graph::build_typescript_family(repo, parses, ts_source).unwrap()
    };
    let g = build();
    let calls = g
        .edges
        .iter()
        .filter(|e| e.category == glia_code_domain::edge_category::CALLS)
        .count();
    assert!(
        calls >= 24 * 6,
        "the receivers, inherited calls and helpers bind: {calls} CALLS"
    );
    same_graph_every_way("typescript", build);
}

#[test]
fn go_graph_is_the_same_pooled() {
    let repo = RepoId(22);
    let files = go_files(16);
    let build = || {
        let parses = files
            .iter()
            .filter(|(path, _)| path.ends_with(".go"))
            .map(|(path, src)| parse_one_with(src, path, "go", repo, "example.com/fx").unwrap())
            .collect();
        glia_graph::build_go(repo, parses).unwrap()
    };
    same_graph_every_way("go", build);
}

#[test]
fn rust_graph_is_the_same_pooled() {
    let repo = RepoId(23);
    let files = rust_files(16);
    let build = || {
        let parses = files
            .iter()
            .filter(|(path, _)| path.ends_with(".rs"))
            .map(|(path, src)| parse_one(src, path, "rust", repo).unwrap())
            .collect();
        glia_graph::build_rust(repo, parses, &[]).unwrap()
    };
    same_graph_every_way("rust", build);
}

/// Map of file name -> bytes for every file in a sharded output dir.
fn dir_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn one_thread_and_sixteen_write_the_same_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    // Large enough that each group's calls cut into several of the
    // engine's chunks (256 sites at the least).
    write_all(&repo, &ts_files(100));
    write_all(&repo, &go_files(120));
    write_all(&repo, &rust_files(120));
    let repo_s = repo.to_str().unwrap();

    let mut outs = Vec::new();
    for n in [1usize, 16] {
        let r = workers(n).install(|| generate_one(repo_s)).unwrap();
        assert!(
            r.parse_errors.is_empty(),
            "{n} threads: {:?}",
            r.parse_errors
        );
        let out = tmp.path().join(format!("out_{n}"));
        write_merged_sharded(&r.merged, &out).unwrap();
        outs.push((n, out));
    }
    let (one, sixteen) = (dir_bytes(&outs[0].1), dir_bytes(&outs[1].1));
    let names = |fs: &[(String, Vec<u8>)]| fs.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(names(&one), names(&sixteen), "1 vs 16 threads: file sets");
    for ((name, a), (_, b)) in one.iter().zip(sixteen.iter()) {
        assert_eq!(a, b, "1 vs 16 threads: {name} bytes differ");
    }
}

// ---------------------------------------------------------------------------
// GP.2: the TypeScript-family group front (merge_parses' per-map folds and
// resolve_imports_ts' decide step) runs on the pool too.
// ---------------------------------------------------------------------------

/// GP.2: the import fixture: a barrel (`export *`, a renamed and a
/// namespace re-export), a barrel of a barrel, a directory index, a default
/// and a namespace import, a tsconfig-style alias, a side-effect import, an
/// import of a missing file and a package import, repeated over `features`
/// importers.
fn ts_import_files(features: usize) -> Vec<(String, String)> {
    let mut files = vec![
        (
            "src/base.ts".to_string(),
            "export class Base {\n  greet(): string { return \"hi\"; }\n}\n\
             export function helper(): number { return 1; }\n\
             export default class Main {\n  start(): void {}\n}\n"
                .to_string(),
        ),
        (
            "src/models/user.ts".to_string(),
            "export class User {\n  name(): string { return \"\"; }\n}\n".to_string(),
        ),
        (
            "src/models/role.ts".to_string(),
            "export class Role {\n  level(): number { return 0; }\n}\n\
             export function grant(): void {}\n"
                .to_string(),
        ),
        (
            "src/models/index.ts".to_string(),
            "export * from './user';\n\
             export { Role as RoleModel } from './role';\n\
             export * as roles from './role';\n"
                .to_string(),
        ),
        (
            "src/lib/index.ts".to_string(),
            "export * from '../models';\nexport { helper } from '../base';\n".to_string(),
        ),
        (
            "src/shared/thing.ts".to_string(),
            "export class Thing {\n  poke(): void {}\n}\n".to_string(),
        ),
        (
            "src/side.ts".to_string(),
            "export const ready = true;\n".to_string(),
        ),
    ];
    for i in 0..features {
        files.push((
            format!("src/feat{i}.ts"),
            format!(
                "import Main, {{ helper }} from './base';\n\
                 import {{ User, RoleModel, roles }} from './models';\n\
                 import {{ User as U2, helper as h2 }} from './lib';\n\
                 import * as base from './base';\n\
                 import {{ Thing }} from '@app/shared/thing';\n\
                 import './side';\n\
                 import {{ nope }} from './missing';\n\
                 import {{ chunk }} from 'lodash';\n\
                 export class Feature{i} {{\n\
                 \x20 go{i}(): void {{\n\
                 \x20   helper();\n\
                 \x20   h2();\n\
                 \x20   base.helper();\n\
                 \x20   new User().name();\n\
                 \x20   new U2().name();\n\
                 \x20   new RoleModel().level();\n\
                 \x20   roles.grant();\n\
                 \x20   new Main().start();\n\
                 \x20   new Thing().poke();\n\
                 \x20   nope();\n\
                 \x20   chunk();\n\
                 \x20 }}\n\
                 }}\n"
            ),
        ));
    }
    files
}

/// GP.2: [`ts_source`] plus the `@app/` alias a tsconfig `paths` entry
/// maps to `src/`.
fn ts_alias_source(from_module: &str, specifier: &str) -> Option<String> {
    let spec = specifier.trim().trim_matches(|c| c == '"' || c == '\'');
    match spec.strip_prefix("@app/") {
        Some(rest) => Some(format!("src::{}", rest.replace('/', "::"))),
        None => ts_source(from_module, specifier),
    }
}

/// GP.2: [`fingerprint`] plus what the front writes beside the edges: the
/// merged nav and the symbol tables, every map sorted.
fn front_fingerprint(g: &RepoGraph) -> String {
    use std::collections::HashMap;
    use std::fmt::Debug;
    // NodeId is Hash, not Ord: entries sort by their Debug text.
    fn sorted<K: Debug, V: Debug>(m: &HashMap<K, V>) -> Vec<String> {
        let mut rows: Vec<String> = m.iter().map(|(k, v)| format!("{k:?} => {v:?}")).collect();
        rows.sort();
        rows
    }
    fn nested<K: Debug, K2: Debug, V: Debug>(m: &HashMap<K, HashMap<K2, V>>) -> Vec<String> {
        let mut rows: Vec<String> = m
            .iter()
            .map(|(k, v)| format!("{k:?} => {:?}", sorted(v)))
            .collect();
        rows.sort();
        rows
    }
    let nav = &g.nav;
    let sy = &g.symbols;
    let mut props: Vec<String> = g.properties.iter().map(|p| format!("{p:?}")).collect();
    props.sort();
    let parts: Vec<Vec<String>> = vec![
        sorted(&nav.name_by_id),
        sorted(&nav.qname_by_id),
        sorted(&nav.kind_by_id),
        sorted(&nav.parent_of),
        sorted(&nav.children_of),
        nested(&nav.field_types),
        nested(&nav.local_types),
        sorted(&nav.nav_facts),
        sorted(&nav.return_types),
        sorted(&nav.method_sigs),
        sorted(&sy.module_by_qname),
        nested(&sy.module_symbols),
        nested(&sy.module_import_bindings),
        nested(&sy.class_methods),
        props,
    ];
    format!("{}\n{parts:#?}", fingerprint(g))
}

#[test]
fn typescript_import_front_is_the_same_pooled() {
    let repo = RepoId(24);
    let files = ts_import_files(40);
    let build = || {
        let parses = files
            .iter()
            .map(|(path, src)| parse_one(src, path, "typescript", repo).unwrap())
            .collect();
        glia_graph::build_typescript_family(repo, parses, ts_alias_source).unwrap()
    };
    let g = build();
    let imports = |rule: &str| {
        g.edges
            .iter()
            .filter(|e| e.category == glia_code_domain::edge_category::IMPORTS)
            .filter(|e| format!("{:?}", e.cells).contains(&format!("\\\"rule\\\":\\\"{rule}\\\"")))
            .count()
    };
    assert!(imports("reexport") >= 3, "the barrels re-export");
    assert!(
        imports("dir_index") >= 40 * 2,
        "./models and ./lib bind their index"
    );
    assert!(
        imports("symbol") >= 40 * 3,
        "named imports bind their module"
    );
    let want = front_fingerprint(&g);
    let pooled = with_resolve_pool(Arc::new(Workers(workers(8))), build);
    assert_eq!(
        front_fingerprint(&pooled),
        want,
        "eight workers, one-statement chunks"
    );
    let backwards = with_resolve_pool(Arc::new(Backwards), build);
    assert_eq!(
        front_fingerprint(&backwards),
        want,
        "one-statement chunks, newest first"
    );
}

// ---------------------------------------------------------------------------
// GP.3: the Go and JVM group fronts. The Go package hook decides on the pool
// (GA.19), and both groups' import tail fallbacks (a path the file layout
// does not mirror, bound by its repo-unique last segment) read one index
// built before the statement loop instead of scanning every module per
// statement.
// ---------------------------------------------------------------------------

/// GP.3: a Go repo exercising every rule the package hook binds through:
/// split-file receivers (a type's methods in another file of its package),
/// a package var's initialiser call, embedded promotion (a promoted method
/// called on the embedder and through `self`), func and method values, a
/// gin group mounted through a parameter, typed receivers, package calls,
/// a tail-fallback import (a path under the module with no directory, bound
/// by its last segment) and calls that bind nothing.
fn go_hook_files(services: usize) -> Vec<(String, String)> {
    let mut files = vec![
        (
            "go.mod".to_string(),
            "module example.com/fx\n\ngo 1.22\n\nrequire github.com/gin-gonic/gin v1.9.1\n"
                .to_string(),
        ),
        (
            "store/store.go".to_string(),
            "package store\n\n\
             type Base struct{}\n\n\
             func (b *Base) Ping() int { return 1 }\n\n\
             type Store struct {\n\tBase\n}\n\n\
             func New() *Store { return &Store{} }\n\n\
             var Default = New()\n"
                .to_string(),
        ),
        (
            "store/store_get.go".to_string(),
            "package store\n\n\
             func (s *Store) Get() int { return s.Ping() + s.local() }\n\n\
             func (s *Store) local() int { return 2 }\n"
                .to_string(),
        ),
        (
            "lib/util/strs.go".to_string(),
            "package strs\n\nfunc Upper(s string) string { return s }\n".to_string(),
        ),
        (
            "api/routes.go".to_string(),
            "package api\n\n\
             import (\n\t\"github.com/gin-gonic/gin\"\n)\n\n\
             func Setup() *gin.Engine {\n\tr := gin.Default()\n\tapi := r.Group(\"/api\")\n\
             \tregister(api)\n\treturn r\n}\n\n\
             func register(g *gin.RouterGroup) {\n\tg.GET(\"/ping\", ping)\n}\n\n\
             func ping(c *gin.Context) {}\n"
                .to_string(),
        ),
    ];
    for i in 0..services {
        files.push((
            format!("svc/svc{i}.go"),
            format!(
                "package svc\n\n\
                 import (\n\t\"example.com/fx/store\"\n\tstrs \"example.com/fx/lib/strs\"\n\
                 \t\"example.com/fx/lib/text\"\n\t\"fmt\"\n)\n\n\
                 type S{i} struct {{\n\tst *store.Store\n}}\n\n\
                 var cache{i} = store.New()\n\n\
                 func (s *S{i}) Run() int {{\n\
                 \tfmt.Println(strs.Upper(\"x\"), text.Upper(\"y\"))\n\
                 \tf := helper{i}\n\
                 \tg := s.Twice\n\
                 \t_ = f\n\t_ = g\n\
                 \treturn s.st.Get() + s.st.Ping() + store.New().Get() + store.Default.Get() + cache{i}.Get() + helper{i}() + s.Twice()\n\
                 }}\n\n\
                 func (s *S{i}) Twice() int {{ return 2 }}\n\n\
                 func helper{i}() int {{ return {i} + gone{i}() }}\n"
            ),
        ));
    }
    files
}

/// GP.3: a Java + Kotlin repo (one JVM graph, `build_dotted`): imports
/// that bind by qname, imports whose package path the file layout does not
/// mirror (bound by the class's repo-unique name, `tail_unique`), a
/// platform import the tail never binds, typed receivers, static and
/// constructor calls, and calls that bind nothing.
fn jvm_files(features: usize) -> Vec<(String, String)> {
    let mut files = vec![
        (
            "src/main/java/com/example/core/Repo.java".to_string(),
            "package com.example.core;\n\n\
             public class Repo {\n    public int load() { return 1; }\n    public static Repo open() { return new Repo(); }\n}\n"
                .to_string(),
        ),
        (
            "lib/Helper.java".to_string(),
            "package com.example.util;\n\n\
             public class Helper {\n    public static int twice(int x) { return x * 2; }\n}\n"
                .to_string(),
        ),
        (
            "kt/Formatter.kt".to_string(),
            "package com.example.fmt\n\n\
             class Formatter {\n    fun format(x: Int): String = x.toString()\n}\n"
                .to_string(),
        ),
    ];
    for i in 0..features {
        files.push((
            format!("src/main/java/com/example/app/Feature{i}.java"),
            format!(
                "package com.example.app;\n\n\
                 import com.example.core.Repo;\n\
                 import com.example.util.Helper;\n\
                 import com.example.fmt.Formatter;\n\
                 import java.util.ArrayList;\n\n\
                 public class Feature{i} {{\n\
                 \x20   private Repo repo = new Repo();\n\
                 \x20   public int run() {{\n\
                 \x20       Formatter f = new Formatter();\n\
                 \x20       f.format({i});\n\
                 \x20       new ArrayList<Integer>();\n\
                 \x20       missing{i}();\n\
                 \x20       return repo.load() + Repo.open().load() + Helper.twice({i});\n\
                 \x20   }}\n\
                 }}\n"
            ),
        ));
        files.push((
            format!("src/main/kotlin/com/example/app/Kt{i}.kt"),
            format!(
                "package com.example.app\n\n\
                 import com.example.core.Repo\n\
                 import com.example.util.Helper\n\
                 import com.example.fmt.Formatter\n\n\
                 class Kt{i}(private val repo: Repo) {{\n\
                 \x20   fun go(): Int {{\n\
                 \x20       Formatter().format({i})\n\
                 \x20       absent{i}()\n\
                 \x20       return repo.load() + Helper.twice({i})\n\
                 \x20   }}\n\
                 }}\n"
            ),
        ));
    }
    files
}

/// The IMPORTS edges of `g` whose EVIDENCE rule is `rule`.
fn imports_by_rule(g: &RepoGraph, rule: &str) -> usize {
    g.edges
        .iter()
        .filter(|e| e.category == glia_code_domain::edge_category::IMPORTS)
        .filter(|e| format!("{:?}", e.cells).contains(&format!("\\\"rule\\\":\\\"{rule}\\\"")))
        .count()
}

/// GP.3: [`same_graph_every_way`] over [`front_fingerprint`], so the import
/// bindings and symbol tables the fronts write are compared too.
fn same_front_every_way(label: &str, build: impl Fn() -> RepoGraph) {
    let sequential = build();
    assert!(
        !sequential.unresolved_calls.is_empty(),
        "{label}: the fixture must leave a call unresolved"
    );
    let want = front_fingerprint(&sequential);
    let pooled = with_resolve_pool(Arc::new(Workers(workers(8))), &build);
    assert_eq!(
        front_fingerprint(&pooled),
        want,
        "{label}: eight workers, one-site chunks"
    );
    let backwards = with_resolve_pool(Arc::new(Backwards), &build);
    assert_eq!(
        front_fingerprint(&backwards),
        want,
        "{label}: one-site chunks run newest first"
    );
}

#[test]
fn go_hook_rules_graph_is_the_same_pooled() {
    let repo = RepoId(25);
    let files = go_hook_files(24);
    let build = || {
        let parses = files
            .iter()
            .filter(|(path, _)| path.ends_with(".go"))
            .map(|(path, src)| parse_one_with(src, path, "go", repo, "example.com/fx").unwrap())
            .collect();
        glia_graph::build_go(repo, parses).unwrap()
    };
    let g = build();
    let rules = |category| -> std::collections::BTreeMap<String, usize> {
        let mut out = std::collections::BTreeMap::new();
        for e in g.edges.iter().filter(|e| e.category == category) {
            let cells = format!("{:?}", e.cells);
            let rule = cells
                .split("\\\"rule\\\":\\\"")
                .nth(1)
                .and_then(|r| r.split("\\\"").next())
                .unwrap_or("-")
                .to_string();
            *out.entry(rule).or_default() += 1;
        }
        out
    };
    let calls = rules(glia_code_domain::edge_category::CALLS);
    let imports = rules(glia_code_domain::edge_category::IMPORTS);
    let uses = rules(glia_code_domain::edge_category::USES);
    let defines = rules(glia_code_domain::edge_category::DEFINES);
    for (what, table, rule) in [
        ("split-file receiver", &defines, "split_receiver"),
        ("promoted self call", &calls, "promoted_self"),
        ("package var receiver", &calls, "receiver_package_var"),
        ("field chain receiver", &calls, "receiver_field_chain"),
        ("func value", &uses, "func_value"),
        ("method value", &uses, "method_value"),
    ] {
        assert!(table.contains_key(rule), "{what} ({rule}) binds: {table:?}");
    }
    assert!(
        g.nav.qname_by_id.values().any(|q| q == "GET /api/ping"),
        "the parameter-held gin group is mounted under /api"
    );
    // `lib/strs` names no directory (the file is lib/util/strs.go), so it
    // binds the repo's one `strs` module by its last segment; no module's
    // last segment is `text`, so `lib/text` binds nothing; `fmt` is the
    // standard library, which the tail never binds.
    assert!(
        imports_by_rule(&g, "tail_unique") >= 24,
        "every service binds lib/strs by its tail: {imports:?}"
    );
    assert!(
        calls.values().sum::<usize>() >= 24 * 6,
        "the package hook binds: {calls:?}"
    );
    same_front_every_way("go hook rules", build);
}

#[test]
fn jvm_graph_is_the_same_pooled() {
    let repo = RepoId(26);
    let files = jvm_files(24);
    let build = || {
        let parses = files
            .iter()
            .map(|(path, src)| {
                let lang = if path.ends_with(".kt") {
                    "kotlin"
                } else {
                    "java"
                };
                parse_one(src, path, lang, repo).unwrap()
            })
            .collect();
        glia_graph::build_dotted(repo, parses).unwrap()
    };
    let g = build();
    assert!(
        imports_by_rule(&g, "tail_unique") >= 24 * 2,
        "Helper and Formatter bind by their repo-unique names"
    );
    same_front_every_way("jvm", build);
}

#[test]
fn go_and_jvm_fronts_write_the_same_layout_on_one_thread_and_sixteen() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    // Enough sites that each group's calls cut into several chunks.
    write_all(&repo, &go_hook_files(80));
    write_all(&repo, &jvm_files(80));
    let repo_s = repo.to_str().unwrap();
    let mut outs = Vec::new();
    for n in [1usize, 16] {
        let r = workers(n).install(|| generate_one(repo_s)).unwrap();
        assert!(
            r.parse_errors.is_empty(),
            "{n} threads: {:?}",
            r.parse_errors
        );
        let out = tmp.path().join(format!("out_{n}"));
        write_merged_sharded(&r.merged, &out).unwrap();
        outs.push(out);
    }
    let (one, sixteen) = (dir_bytes(&outs[0]), dir_bytes(&outs[1]));
    let names = |fs: &[(String, Vec<u8>)]| fs.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(names(&one), names(&sixteen), "1 vs 16 threads: file sets");
    for ((name, a), (_, b)) in one.iter().zip(sixteen.iter()) {
        assert_eq!(a, b, "1 vs 16 threads: {name} bytes differ");
    }
}
