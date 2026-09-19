//! LB.6 — move-stable identity over hand-built graphs: the hint round-trip,
//! every `rebind` tier (Exact / Moved SameName / Moved Identical / Ambiguous /
//! Orphan), `detect_moves` on a synthetic prior/current pair, a declared rename
//! beating a basename match, an unrelated same-name file being rejected, and
//! `carry_file_tokens` keeping a first-sight token across two moves.

use std::collections::{BTreeMap, HashSet};

use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Node, NodeId, NodeKindId, RepoId};
use glia_graph::identity::{
    FileMove, Identity, IdentityIndex, MoveMap, MoveTier, Rebind, carry_file_tokens, detect_moves,
    detect_moves_with, identity_of,
};
use glia_graph::{MergedGraph, RepoGraph, SymbolTable};

fn repo() -> RepoId {
    RepoId::from_canonical("test://identity_moves")
}

fn id(kind: NodeKindId, qname: &str) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
}

/// One declaration: its kind, qname, parent qname (`""` = none) and CODE text.
/// Every node gets a POSITION cell for `file`.
struct Decl<'a> {
    kind: NodeKindId,
    qname: &'a str,
    parent: Option<(NodeKindId, &'a str)>,
    code: &'a str,
}

fn decl<'a>(
    kind: NodeKindId,
    qname: &'a str,
    parent: Option<(NodeKindId, &'a str)>,
    code: &'a str,
) -> Decl<'a> {
    Decl {
        kind,
        qname,
        parent,
        code,
    }
}

/// A file: its repo-relative path and its declarations, the MODULE first.
fn file<'a>(path: &'a str, decls: Vec<Decl<'a>>) -> (&'a str, Vec<Decl<'a>>) {
    (path, decls)
}

fn graph(files: Vec<(&str, Vec<Decl<'_>>)>) -> MergedGraph {
    let r = repo();
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    for (path, decls) in files {
        for d in decls {
            let nid = id(d.kind, d.qname);
            let mut cells = vec![Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(format!(
                    "{{\"file\":\"{path}\",\"start_line\":0,\"end_line\":1}}"
                )),
            }];
            if !d.code.is_empty() {
                cells.push(Cell {
                    kind: cell_type::CODE,
                    payload: CellPayload::Text(d.code.to_string()),
                });
            }
            nodes.push(Node {
                id: nid,
                repo: r,
                confidence: Confidence::Strong,
                cells,
            });
            let name = d.qname.rsplit("::").next().unwrap_or(d.qname);
            nav.record(nid, name, d.qname, d.kind, d.parent.map(|(k, q)| id(k, q)));
        }
    }
    MergedGraph::new(vec![RepoGraph {
        repo: r,
        nodes,
        edges: vec![],
        nav,
        symbols: SymbolTable::default(),
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: HashSet::new(),
    }])
}

const M: NodeKindId = node_kind::MODULE;
const F: NodeKindId = node_kind::FUNCTION;
const C: NodeKindId = node_kind::CLASS;
const ME: NodeKindId = node_kind::METHOD;

/// Long enough (>= 40 chars after whitespace collapse) to carry a body hash.
const BIG_BODY: &str =
    "def compute_totals(rows):\n    return sum(r.amount for r in rows if r.ok)\n";

/// `a/users.py`: class User { get }, fn load. `lib/big.py`: fn compute_totals.
fn prior() -> MergedGraph {
    graph(vec![
        file(
            "a/users.py",
            vec![
                decl(M, "a::users", None, "class User: ...\ndef load(): ...\n"),
                decl(
                    C,
                    "a::users::User",
                    Some((M, "a::users")),
                    "class User: ...",
                ),
                decl(
                    ME,
                    "a::users::User::get",
                    Some((C, "a::users::User")),
                    "def get(self): ...",
                ),
                decl(
                    F,
                    "a::users::load",
                    Some((M, "a::users")),
                    "def load(): ...",
                ),
            ],
        ),
        file(
            "lib/big.py",
            vec![
                decl(M, "lib::big", None, BIG_BODY),
                decl(
                    F,
                    "lib::big::compute_totals",
                    Some((M, "lib::big")),
                    BIG_BODY,
                ),
            ],
        ),
    ])
}

/// `a/users.py` moved to `b/users.py` with one function added; `lib/big.py`
/// moved AND renamed to `lib/sums.py`, byte-identical.
fn current() -> MergedGraph {
    graph(vec![
        file(
            "b/users.py",
            vec![
                decl(
                    M,
                    "b::users",
                    None,
                    "class User: ...\ndef load(): ...\ndef save(): ...\n",
                ),
                decl(
                    C,
                    "b::users::User",
                    Some((M, "b::users")),
                    "class User: ...",
                ),
                decl(
                    ME,
                    "b::users::User::get",
                    Some((C, "b::users::User")),
                    "def get(self): ...",
                ),
                decl(
                    F,
                    "b::users::load",
                    Some((M, "b::users")),
                    "def load(): ...",
                ),
                decl(
                    F,
                    "b::users::save",
                    Some((M, "b::users")),
                    "def save(): ...",
                ),
            ],
        ),
        file(
            "lib/sums.py",
            vec![
                decl(M, "lib::sums", None, BIG_BODY),
                decl(
                    F,
                    "lib::sums::compute_totals",
                    Some((M, "lib::sums")),
                    BIG_BODY,
                ),
            ],
        ),
    ])
}

#[test]
fn hint_round_trips_including_separator_characters() {
    let plain = Identity {
        kind: ME,
        file: "users.py".into(),
        local: "User::get".into(),
        body: 0xdead_beef,
    };
    assert_eq!(
        plain.hint(),
        format!("v1|{}|users.py|00000000deadbeef|User::get", ME.0)
    );
    assert_eq!(Identity::parse_hint(&plain.hint()), Some(plain));

    let odd = Identity {
        kind: M,
        file: "we|rd%name.py".into(),
        local: "".into(),
        body: u64::MAX,
    };
    assert_eq!(Identity::parse_hint(&odd.hint()), Some(odd));

    assert_eq!(Identity::parse_hint("garbage"), None);
    assert_eq!(
        Identity::parse_hint("v0|4|users.py|0000000000000000|x"),
        None
    );
    assert_eq!(Identity::parse_hint("v1|4|users.py|nothex|x"), None);
}

#[test]
fn identity_is_derived_from_nav_position_and_code() {
    let g = prior();
    let m = identity_of(&g, id(ME, "a::users::User::get")).expect("method has a MODULE ancestor");
    assert_eq!(
        (m.kind, m.file.as_str(), m.local.as_str()),
        (ME, "users.py", "User::get")
    );
    assert_eq!(m.body, 0, "a body under 40 collapsed chars carries no hash");

    let module = identity_of(&g, id(M, "a::users")).expect("a MODULE is its own anchor");
    assert_eq!(module.local, "");

    let big = identity_of(&g, id(F, "lib::big::compute_totals")).expect("function");
    assert_ne!(big.body, 0);
    // Whitespace runs collapse: re-indenting the body keeps the hash.
    let reindented = graph(vec![file(
        "x/big.py",
        vec![
            decl(M, "x::big", None, ""),
            decl(
                F,
                "x::big::compute_totals",
                Some((M, "x::big")),
                "def compute_totals(rows):\n\n        return sum(r.amount   for r in rows if r.ok)",
            ),
        ],
    )]);
    let re = identity_of(&reindented, id(F, "x::big::compute_totals")).expect("function");
    assert_eq!(re.body, big.body);

    // A node with no MODULE ancestor has no path-derived identity.
    let orphan = graph(vec![file(
        "r.py",
        vec![decl(node_kind::ROUTE, "GET /x", None, "")],
    )]);
    assert_eq!(identity_of(&orphan, id(node_kind::ROUTE, "GET /x")), None);
    assert_eq!(identity_of(&g, NodeId(42)), None);
}

#[test]
fn rebind_walks_every_tier() {
    let before = prior();
    let after = current();
    let idx = IdentityIndex::build(&after);

    // Exact: the qname still exists.
    assert_eq!(
        idx.rebind("b::users::load", Some(F), None),
        Rebind::Exact(id(F, "b::users::load"))
    );
    assert_eq!(
        idx.rebind("b::users::load", None, None),
        Rebind::Exact(id(F, "b::users::load"))
    );

    // Moved{SameName}: basename + local path survive the move.
    let hint = identity_of(&before, id(ME, "a::users::User::get"))
        .expect("method")
        .hint();
    assert_eq!(
        idx.rebind("a::users::User::get", Some(ME), Some(&hint)),
        Rebind::Moved {
            id: id(ME, "b::users::User::get"),
            tier: MoveTier::SameName
        }
    );

    // Moved{Identical}: the file was renamed, only the body hash matches.
    let hint = identity_of(&before, id(F, "lib::big::compute_totals"))
        .expect("fn")
        .hint();
    assert_eq!(
        idx.rebind("lib::big::compute_totals", Some(F), Some(&hint)),
        Rebind::Moved {
            id: id(F, "lib::sums::compute_totals"),
            tier: MoveTier::Identical
        }
    );

    // Orphan: no hint, or a hint nothing matches.
    assert_eq!(idx.rebind("gone::x", Some(F), None), Rebind::Orphan);
    let stray = Identity {
        kind: F,
        file: "nope.py".into(),
        local: "zzz".into(),
        body: 0,
    };
    assert_eq!(
        idx.rebind("gone::x", Some(F), Some(&stray.hint())),
        Rebind::Orphan
    );
    // A hint whose kind contradicts the caller's is not trusted.
    let hint = identity_of(&before, id(ME, "a::users::User::get"))
        .expect("method")
        .hint();
    assert_eq!(
        idx.rebind("a::users::User::get", Some(F), Some(&hint)),
        Rebind::Orphan
    );

    // Ambiguous: two `index.ts` modules share (MODULE, "index.ts", "").
    let two = graph(vec![
        file(
            "web/index.ts",
            vec![decl(M, "web::index", None, "export * from './a';")],
        ),
        file(
            "api/index.ts",
            vec![decl(M, "api::index", None, "export * from './b';")],
        ),
    ]);
    let old = Identity {
        kind: M,
        file: "index.ts".into(),
        local: "".into(),
        body: 0,
    };
    let two_idx = IdentityIndex::build(&two);
    let mut both = vec![id(M, "web::index"), id(M, "api::index")];
    both.sort_by_key(|n| n.0);
    assert_eq!(
        two_idx.rebind("lib::index", Some(M), Some(&old.hint())),
        Rebind::Ambiguous(both)
    );
    assert!(!two_idx.is_unique(&old));
    let web = identity_of(&two, id(M, "web::index")).expect("module");
    assert!(
        !two_idx.is_unique(&web),
        "same (kind, file, local) key as api/index.ts"
    );
    let get = identity_of(&after, id(ME, "b::users::User::get")).expect("method");
    assert!(idx.is_unique(&get));
}

#[test]
fn detect_moves_pairs_files_and_aligns_descendants() {
    let map = detect_moves(&prior(), &current());
    assert_eq!(
        map.files,
        vec![
            FileMove {
                old_path: "a/users.py".into(),
                new_path: "b/users.py".into(),
                tier: MoveTier::SameName,
            },
            FileMove {
                old_path: "lib/big.py".into(),
                new_path: "lib/sums.py".into(),
                tier: MoveTier::Identical,
            },
        ]
    );
    assert_eq!(map.rejected, 0);
    let pairs: Vec<(&str, &str)> = map
        .nodes
        .iter()
        .map(|n| (n.old_qname.as_str(), n.new_qname.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("a::users", "b::users"),
            ("a::users::User", "b::users::User"),
            ("a::users::User::get", "b::users::User::get"),
            ("a::users::load", "b::users::load"),
            ("lib::big", "lib::sums"),
            ("lib::big::compute_totals", "lib::sums::compute_totals"),
        ]
    );
    let get = &map.nodes[2];
    assert_eq!(
        (get.kind, get.old_id, get.new_id),
        (
            ME,
            id(ME, "a::users::User::get"),
            id(ME, "b::users::User::get")
        )
    );
    assert!(
        !map.nodes.iter().any(|n| n.new_qname == "b::users::save"),
        "a real addition is not a move"
    );
    // Pure and deterministic.
    assert_eq!(detect_moves(&prior(), &current()), map);
}

/// `a/x.py` (f, g) with both `b/x.py` (f, g) and `c/y.py` (f, g) added: the
/// basename rule would pick `b/x.py`; a declared rename to `c/y.py` is a FACT
/// and wins.
#[test]
fn declared_rename_beats_a_basename_match() {
    let before = graph(vec![file(
        "a/x.py",
        vec![
            decl(M, "a::x", None, "def f(): ...\ndef g(): ...\n# old"),
            decl(F, "a::x::f", Some((M, "a::x")), ""),
            decl(F, "a::x::g", Some((M, "a::x")), ""),
        ],
    )]);
    let after = graph(vec![
        file(
            "b/x.py",
            vec![
                decl(M, "b::x", None, "def f(): ...\ndef g(): ...\n# b"),
                decl(F, "b::x::f", Some((M, "b::x")), ""),
                decl(F, "b::x::g", Some((M, "b::x")), ""),
            ],
        ),
        file(
            "c/y.py",
            vec![
                decl(M, "c::y", None, "def f(): ...\ndef g(): ...\n# c"),
                decl(F, "c::y::f", Some((M, "c::y")), ""),
                decl(F, "c::y::g", Some((M, "c::y")), ""),
            ],
        ),
    ]);
    let guessed = detect_moves(&before, &after);
    assert_eq!(
        guessed.files,
        vec![FileMove {
            old_path: "a/x.py".into(),
            new_path: "b/x.py".into(),
            tier: MoveTier::SameName
        }]
    );
    let declared = detect_moves_with(&before, &after, &[("a/x.py".into(), "./c/y.py".into())]);
    assert_eq!(
        declared.files,
        vec![FileMove {
            old_path: "a/x.py".into(),
            new_path: "c/y.py".into(),
            tier: MoveTier::Declared
        }]
    );
    assert_eq!(declared.nodes.len(), 3);
    assert!(
        declared
            .nodes
            .iter()
            .all(|n| n.new_qname.starts_with("c::y"))
    );
}

#[test]
fn unrelated_same_basename_file_is_rejected() {
    let before = graph(vec![file(
        "a/x.py",
        vec![
            decl(M, "a::x", None, "def f(): ...\ndef g(): ...\n"),
            decl(F, "a::x::f", Some((M, "a::x")), ""),
            decl(F, "a::x::g", Some((M, "a::x")), ""),
        ],
    )]);
    let after = graph(vec![file(
        "b/x.py",
        vec![
            decl(M, "b::x", None, "def h(): ...\n"),
            decl(F, "b::x::h", Some((M, "b::x")), ""),
        ],
    )]);
    let map = detect_moves(&before, &after);
    assert_eq!(
        map,
        MoveMap {
            files: vec![],
            nodes: vec![],
            rejected: 1
        }
    );
}

#[test]
fn carry_file_tokens_keeps_first_sight_token_across_two_moves() {
    let step = |old: &str, new: &str| MoveMap {
        files: vec![FileMove {
            old_path: old.into(),
            new_path: new.into(),
            tier: MoveTier::Identical,
        }],
        nodes: vec![],
        rejected: 0,
    };
    let first: Vec<String> = vec!["a.py".into(), "keep.py".into()];
    let t0 = carry_file_tokens(&BTreeMap::new(), &MoveMap::default(), &first);
    assert_eq!(t0.get("a.py").map(String::as_str), Some("a.py"));
    assert_eq!(t0.get("keep.py").map(String::as_str), Some("keep.py"));

    let t1 = carry_file_tokens(
        &t0,
        &step("a.py", "b.py"),
        &["b.py".into(), "keep.py".into()],
    );
    let t2 = carry_file_tokens(
        &t1,
        &step("b.py", "c.py"),
        &["c.py".into(), "keep.py".into()],
    );
    let want: BTreeMap<String, String> = [("c.py", "a.py"), ("keep.py", "keep.py")]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
    assert_eq!(t2, want);

    // A new file created at a vacated path is a different file: it must not
    // share the moved file's token.
    let t3 = carry_file_tokens(&t0, &step("a.py", "b.py"), &["a.py".into(), "b.py".into()]);
    assert_eq!(t3.get("b.py").map(String::as_str), Some("a.py"));
    let fresh = t3.get("a.py").cloned().unwrap_or_default();
    assert_ne!(fresh, "a.py");
    assert!(!fresh.is_empty());
}
