//! LD.7c — `implementors`: who implements or extends a type (or overrides a
//! method), transitively, and its supertypes, each row tiered by the weakest
//! heritage edge on its path. The graph has held the edges since LD.7a / LD.7b
//! / A6.6; before this the only route was a blast radius, which mixes them
//! with CALLS / INJECTS rows and gives no tier, and says nothing at all for a
//! PHP interface (no heritage edge is extracted there).

use repo_graph_engine::generate_one;
use repo_graph_engine::implementors::{HierarchyDirection, Implementor, implementors};
use repo_graph_graph::MergedGraph;

use HierarchyDirection::{Down, Up};

const CSHARP: &str = "namespace Shop {\n    public interface IRepo { string Get(string id); }\n    public interface IUserRepo : IRepo { string ByEmail(string email); }\n    public class PgUserRepo : IUserRepo {\n        public string Get(string id) { return id; }\n        public string ByEmail(string email) { return email; }\n    }\n}\n";

const TS: &str = "export interface Repo {\n  get(id: string): string;\n}\n\nexport interface UserRepo extends Repo {\n  byEmail(email: string): string;\n}\n\nexport class PgUserRepo implements UserRepo {\n  get(id: string): string { return id; }\n  byEmail(email: string): string { return email; }\n}\n\nexport class CachedPgUserRepo extends PgUserRepo {\n  get(id: string): string { return super.get(id); }\n}\n";

/// LD.7a's java-iface-extends sources: each type in its own file, so each
/// file MODULE shares its type's name and qname.
const JAVA: [(&str, &str); 3] = [
    (
        "Readable.java",
        "package shop;\n\npublic interface Readable {\n    String read(String id);\n}\n",
    ),
    (
        "Catalog.java",
        "package shop;\n\npublic interface Catalog extends Readable {\n    String search(String q);\n}\n",
    ),
    (
        "PgCatalog.java",
        "package shop;\n\npublic class PgCatalog implements Catalog {\n    public String read(String id) { return id; }\n    public String search(String q) { return q; }\n}\n",
    ),
];

/// LD.7b's go-implicit-iface sources: MemStore satisfies Store without naming
/// it; ReadOnly has Get but not Put.
const GO: [(&str, &str); 3] = [
    ("go.mod", "module example.com/shop\n\ngo 1.22\n"),
    (
        "store.go",
        "package shop\n\n// Store is satisfied implicitly: no type names it.\ntype Store interface {\n\tGet(id string) (string, error)\n\tPut(id string, v string) error\n}\n\nfunc Use(s Store) {\n\ts.Get(\"x\")\n}\n",
    ),
    (
        "mem.go",
        "package shop\n\ntype MemStore struct{ data map[string]string }\n\nfunc (m *MemStore) Get(id string) (string, error) { return m.data[id], nil }\n\nfunc (m *MemStore) Put(id string, v string) error {\n\tm.data[id] = v\n\treturn nil\n}\n\n// ReadOnly has Get but not Put, so it does not satisfy Store.\ntype ReadOnly struct{}\n\nfunc (r ReadOnly) Get(id string) (string, error) { return \"\", nil }\n",
    ),
];

const PHP: &str = "<?php\nnamespace Shop;\ninterface Readable { public function read(string $id): string; }\ninterface Catalog extends Readable { public function search(string $q): array; }\nclass PgCatalog implements Catalog {\n    public function read(string $id): string { return $id; }\n    public function search(string $q): array { return [$q]; }\n}\n";

fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (name, src) in files {
        std::fs::write(tmp.path().join(name), src).expect("write source");
    }
    let merged = generate_one(tmp.path().to_str().expect("utf-8 temp path"))
        .expect("generate_one")
        .merged;
    (tmp, merged)
}

/// `(qname, depth, relation, tier, via)` of every row, in answer order.
fn rows(rs: &[Implementor]) -> Vec<(&str, usize, &str, &str, Option<&str>)> {
    rs.iter()
        .map(|r| {
            (
                r.qname.as_str(),
                r.depth,
                r.relation,
                r.tier,
                r.via.as_deref(),
            )
        })
        .collect()
}

fn qnames(rs: &[Implementor]) -> Vec<&str> {
    rs.iter().map(|r| r.qname.as_str()).collect()
}

#[test]
fn csharp_interface_chain_down_up_and_direct() {
    let (_tmp, m) = build(&[("Repos.cs", CSHARP)]);

    let down = implementors(&m, "Shop::IRepo", Down, true);
    assert!(down.absence.is_none());
    assert_eq!(
        rows(&down.results),
        [
            ("Shop::IUserRepo", 1, "IMPLEMENTS", "FACT", None),
            (
                "Shop::PgUserRepo",
                2,
                "IMPLEMENTS",
                "FACT",
                Some("Shop::IUserRepo")
            ),
        ],
        "C# emits interface-to-interface heritage as IMPLEMENTS; both are declared"
    );
    let pg = &down.results[1];
    assert_eq!(
        (pg.kind, pg.file.as_deref(), pg.line),
        ("CLASS", Some("Repos.cs"), Some(4))
    );

    let direct = implementors(&m, "Shop::IRepo", Down, false);
    assert_eq!(qnames(&direct.results), ["Shop::IUserRepo"]);

    let up = implementors(&m, "Shop::PgUserRepo", Up, true);
    assert_eq!(qnames(&up.results), ["Shop::IUserRepo", "Shop::IRepo"]);
    assert_eq!(up.results[1].via.as_deref(), Some("Shop::IUserRepo"));
}

#[test]
fn csharp_method_target_walks_its_owner_hierarchy() {
    // PgUserRepo implements IRepo::Get only through IUserRepo: A6.6 pairs
    // methods over a direct type-level edge, so no method-level edge joins
    // them, and the owner walk carries the pair through the hierarchy.
    let (_tmp, m) = build(&[("Repos.cs", CSHARP)]);
    let down = implementors(&m, "Shop::IRepo::Get", Down, true);
    assert_eq!(
        rows(&down.results),
        [(
            "Shop::PgUserRepo::Get",
            2,
            "IMPLEMENTS",
            "FACT",
            Some("Shop::IUserRepo")
        )]
    );
    assert_eq!(down.results[0].kind, "METHOD");
    // Direct only: IRepo's direct implementor, IUserRepo, declares no Get.
    let direct = implementors(&m, "Shop::IRepo::Get", Down, false);
    assert!(direct.results.is_empty());
    assert_eq!(direct.absence.map(|a| a.reason), Some("no_edges"));

    let up = implementors(&m, "Shop::PgUserRepo::Get", Up, true);
    assert_eq!(qnames(&up.results), ["Shop::IRepo::Get"]);
    // A6.6's own method-level edge (PgUserRepo::ByEmail -> IUserRepo::ByEmail)
    // is the same answer from the other side.
    let by_email = implementors(&m, "Shop::IUserRepo::ByEmail", Down, true);
    assert_eq!(qnames(&by_email.results), ["Shop::PgUserRepo::ByEmail"]);
}

#[test]
fn typescript_class_chain_and_override() {
    let (_tmp, m) = build(&[("repo.ts", TS)]);

    let sub = implementors(&m, "repo::PgUserRepo", Down, true);
    assert_eq!(
        rows(&sub.results),
        [("repo::CachedPgUserRepo", 1, "INHERITS_FROM", "FACT", None)]
    );

    let user = implementors(&m, "repo::UserRepo", Down, true);
    assert_eq!(
        rows(&user.results),
        [
            ("repo::PgUserRepo", 1, "IMPLEMENTS", "FACT", None),
            (
                "repo::CachedPgUserRepo",
                2,
                "INHERITS_FROM",
                "FACT",
                Some("repo::PgUserRepo")
            ),
        ]
    );
    // Interface-extends (LD.7a) puts both below Repo.
    let repo = implementors(&m, "repo::Repo", Down, true);
    assert_eq!(
        qnames(&repo.results),
        [
            "repo::UserRepo",
            "repo::PgUserRepo",
            "repo::CachedPgUserRepo"
        ]
    );

    // A class method's override along INHERITS_FROM.
    let get = implementors(&m, "repo::PgUserRepo::get", Down, true);
    assert_eq!(
        rows(&get.results),
        [(
            "repo::CachedPgUserRepo::get",
            1,
            "INHERITS_FROM",
            "FACT",
            None
        )]
    );
}

#[test]
fn java_target_resolves_the_interface_not_its_file_module() {
    let (_tmp, m) = build(&JAVA);
    // `Readable` names both Readable.java's MODULE and the INTERFACE; the
    // MODULE has no heritage edge, so rows prove the INTERFACE was taken.
    let a = implementors(&m, "Readable", Down, true);
    assert_eq!(
        rows(&a.results),
        [
            ("Catalog", 1, "INHERITS_FROM", "FACT", None),
            ("PgCatalog", 2, "IMPLEMENTS", "FACT", Some("Catalog")),
        ]
    );
    assert!(a.results.iter().all(|r| r.kind != "MODULE"));
    assert_eq!(a.results[1].file.as_deref(), Some("PgCatalog.java"));
}

#[test]
fn go_implicit_satisfaction_is_derived() {
    let (_tmp, m) = build(&GO);

    let store = implementors(&m, "store::Store", Down, true);
    assert_eq!(
        rows(&store.results),
        [("mem::MemStore", 1, "IMPLEMENTS", "DERIVED", None)],
        "inferred by method-name set (Medium); ReadOnly lacks Put"
    );

    // The method pair A6.6 stamps Strong rides on a Medium type-level edge:
    // the owner walk gives it that edge's tier.
    let get = implementors(&m, "store::Store::Get", Down, true);
    assert_eq!(
        rows(&get.results),
        [("mem::MemStore::Get", 1, "IMPLEMENTS", "DERIVED", None)]
    );
    let up = implementors(&m, "mem::MemStore::Get", Up, true);
    assert_eq!(qnames(&up.results), ["store::Store::Get"]);
    assert_eq!(up.results[0].tier, "DERIVED");
    let read_only = implementors(&m, "mem::ReadOnly", Up, true);
    assert!(read_only.results.is_empty());
}

#[test]
fn php_heritage_is_blind_and_the_absence_says_so() {
    let (_tmp, m) = build(&[("Repo.php", PHP)]);
    // PHP qnames carry no namespace at HEAD: the interface is `Catalog`.
    let a = implementors(&m, "Catalog", Down, true);
    assert!(a.results.is_empty());
    let absence = a.absence.expect("empty answer carries an absence");
    assert_eq!((absence.tier, absence.reason), ("FACT", "no_edges"));
    assert_eq!(absence.mechanisms, ["IMPLEMENTS", "INHERITS_FROM"]);
    assert!(
        absence.note.contains("`Catalog` (INTERFACE)"),
        "{}",
        absence.note
    );
    let php: Vec<_> = absence
        .caveats
        .iter()
        .map(|c| (c.language, c.edge_category))
        .collect();
    assert_eq!(php, [("php", "IMPLEMENTS"), ("php", "INHERITS_FROM")]);
}

#[test]
fn an_unknown_symbol_is_unknown_and_another_kind_is_walked() {
    let (_tmp, m) = build(&[("Repos.cs", CSHARP)]);
    let a = implementors(&m, "IUsrRepo", Down, true);
    assert!(a.results.is_empty());
    let absence = a.absence.expect("absence");
    assert_eq!(absence.reason, "unknown_symbol");
    // A typo: no exact tier, so find's nearest qnames are the suggestions.
    assert_eq!(
        absence.suggestions.first().map(String::as_str),
        Some("Shop::IUserRepo")
    );

    // `Repos` exactly names the file MODULE: it exists, so the answer is
    // no_edges about it, never "no node has that name".
    let module = implementors(&m, "Repos", Down, true);
    let absence = module.absence.expect("absence");
    assert_eq!(absence.reason, "no_edges");
    assert!(absence.note.contains("(MODULE)"), "{}", absence.note);
}
