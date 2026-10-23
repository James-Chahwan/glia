//! EB.5c: `intent::paths::PathResolver`, rank_eval's `KPaths`, over inline
//! candidates and renames (no build, no snapshot).

use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, HistorySnapshot};
use glia_engine::intent::paths::{DIR_CAP, PTok, PathResolver, norm, rename_pairs};
use glia_engine::intent::text::Candidates;

fn cands(paths: &[&str]) -> Candidates {
    let mut c = Candidates::default();
    let mut v: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
    v.sort();
    c.paths = v;
    c.deg = vec![0.0; c.paths.len()];
    c.files = (0..c.paths.len() as u32).collect();
    c
}

fn renames(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

fn toks(t: &[&str]) -> Vec<String> {
    t.iter().map(|s| s.to_string()).collect()
}

/// (path, route) per resolved row, in the block's output order.
fn block<'a>(r: &'a PathResolver, t: &[&str]) -> Vec<(&'a str, &'static str)> {
    r.block(&toks(t)).into_iter().map(|(f, route, _)| (r.candidates()[f].as_str(), route)).collect()
}

const CANDS: &[&str] = &[
    "README.md",
    "src/a.rs",
    "src/b.rs",
    "src/c.ts",
    "src/billing/export.rs",
    "src/billing/mod.rs",
    "src/ledger/export.rs",
    "src/ledger/mod.rs",
    "lib/util/export.rs",
    "docs/guide.md",
    "web/app/main.ts",
    "web/app/api/client.ts",
    "engine/src/intent/paths.rs",
    "engine/src/intent/text.rs",
    "engine/src/rank/files.rs",
    "tools/x.py",
];

const RENAMES: &[(&str, &str)] = &[
    ("rust/engine/src/old.rs", "engine/src/mid.rs"),
    ("engine/src/mid.rs", "engine/src/rank/files.rs"),
    ("old/ui/main.ts", "web/app/main.ts"),
    ("gone/x.rs", "gone/y.rs"),
];

fn resolver() -> PathResolver {
    PathResolver::new(&cands(CANDS), &renames(RENAMES))
}

#[test]
fn exact_name_and_context() {
    let r = PathResolver::new(&cands(&["src/billing/export.rs", "src/billing/mod.rs", "lib/x.rs"]), &[]);
    assert_eq!(r.file("src/billing/export.rs", &[]), Some((r.map("src/billing/export.rs").unwrap(), "exact")));
    assert_eq!(block(&r, &["export.rs"]), [("src/billing/export.rs", "name")]);
    assert_eq!(block(&r, &["billing/export.rs"]), [("src/billing/export.rs", "suffix")]);

    let r = resolver();
    // two export.rs share no directory with an empty block: ambiguous
    assert_eq!(block(&r, &["export.rs"]), []);
    assert_eq!(block(&r, &["export.rs", "src/billing/mod.rs"]), [("src/billing/export.rs", "context"), ("src/billing/mod.rs", "exact")]);
}

#[test]
fn renamed_twice_resolves_to_final_candidate() {
    let r = resolver();
    // rust/engine/src/old.rs -> engine/src/mid.rs -> engine/src/rank/files.rs
    assert_eq!(block(&r, &["rust/engine/src/old.rs"]), [("engine/src/rank/files.rs", "rename")]);
    assert_eq!(block(&r, &["engine/src/mid.rs"]), [("engine/src/rank/files.rs", "rename")]);
    assert_eq!(block(&r, &["old.rs"]), [("engine/src/rank/files.rs", "rename-suffix")]);
    // a chain ending outside the candidates maps nothing
    assert_eq!(r.map("gone/x.rs"), None);
    assert_eq!(r.renamed(), 3);
}

#[test]
fn directories_and_cap() {
    let r = resolver();
    assert_eq!(block(&r, &["billing/"]), [("src/billing/export.rs", "dir-suffix"), ("src/billing/mod.rs", "dir-suffix")]);
    let many: Vec<String> = (0..DIR_CAP + 1).map(|i| format!("big/f{i:02}.rs")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let r = PathResolver::new(&cands(&refs), &[]);
    assert_eq!(r.dir("big"), None);
    assert_eq!(block(&r, &["big/"]), []);
    let line = r.report_line("t", "test");
    assert!(line.contains(" over_cap=2 "), "{line}");
    let at_cap: Vec<&str> = refs[..DIR_CAP].to_vec();
    let r = PathResolver::new(&cands(&at_cap), &[]);
    assert_eq!(r.dir("big").map(|(v, route)| (v.len(), route)), Some((DIR_CAP, "dir")));
}

#[test]
fn braces_and_globs() {
    let r = resolver();
    assert_eq!(block(&r, &["src/{a,b}.rs"]), [("src/a.rs", "glob"), ("src/b.rs", "glob")]);
    let many: Vec<String> = (0..40).map(|i| format!("src/m{i:02}/x.rs")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let r = PathResolver::new(&cands(&refs), &[]);
    assert_eq!(block(&r, &["src/**/*.rs"]), []);
    assert!(r.report_line("t", "test").contains(" over_cap=1 "));
    let r = PathResolver::new(&cands(&refs[..DIR_CAP]), &[]);
    assert_eq!(block(&r, &["src/**/*.rs"]).len(), DIR_CAP);
}

#[test]
fn normalisation_line_ranges_and_drops() {
    let r = resolver();
    assert_eq!(block(&r, &["src/a.rs:12-40"]), [("src/a.rs", "exact")]);
    assert_eq!(block(&r, &["src/a.rs#L12"]), [("src/a.rs", "exact")]);
    assert_eq!(block(&r, &["https://x/y.rs", "../../etc"]), []);
    assert_eq!(norm("https://x/y.rs", r.exts()), None);
    assert_eq!(norm("../../etc", r.exts()), None);
    assert_eq!(norm("./src/a.rs,", r.exts()), Some(PTok::File("src/a.rs".into())));
    assert_eq!(norm("**src/{a,b}.rs**", r.exts()), Some(PTok::Glob("src/{a,b}.rs".into())));
    assert_eq!(norm("billing/", r.exts()), Some(PTok::Dir("billing".into())));
    assert_eq!(norm("user@host", r.exts()), None);
}

/// A block's tokens and its expected (path, route) rows.
type Case<'a> = (&'a [&'a str], &'a [(&'a str, &'a str)]);

/// rank_eval `KPaths::block` over these candidates and renames, run from the
/// harness's own code (KPaths, kpath_norm, kbraces, kglob copied verbatim
/// into a standalone program), its (file, route) rows sorted and deduped.
#[test]
fn parity_with_rank_eval_kpaths() {
    let r = resolver();
    let cases: &[Case] = &[
        (&["src/billing/export.rs"], &[("src/billing/export.rs", "exact")]),
        (&["export.rs"], &[]),
        (&["export.rs", "src/billing/mod.rs"], &[("src/billing/export.rs", "context"), ("src/billing/mod.rs", "exact")]),
        (&["rust/engine/src/old.rs"], &[("engine/src/rank/files.rs", "rename")]),
        (&["old.rs"], &[("engine/src/rank/files.rs", "rename-suffix")]),
        (&["billing/"], &[("src/billing/export.rs", "dir-suffix"), ("src/billing/mod.rs", "dir-suffix")]),
        (&["src/{a,b}.rs"], &[("src/a.rs", "glob"), ("src/b.rs", "glob")]),
        (&["`src/a.rs:12-40`"], &[]),
        (&["src/a.rs#L12"], &[("src/a.rs", "exact")]),
        (&["https://x/y.rs", "../../etc", "www.foo.rs"], &[]),
        (
            &["./web/app/", "**src/c.ts**", "intent/"],
            &[
                ("engine/src/intent/paths.rs", "dir-suffix"),
                ("engine/src/intent/text.rs", "dir-suffix"),
                ("src/c.ts", "exact"),
                ("web/app/api/client.ts", "dir"),
                ("web/app/main.ts", "dir"),
            ],
        ),
        (
            &["engine/src/**/*.rs", "*.md", "app/*.ts"],
            &[
                ("engine/src/intent/paths.rs", "glob"),
                ("engine/src/intent/text.rs", "glob"),
                ("engine/src/rank/files.rs", "glob"),
                ("web/app/main.ts", "glob"),
            ],
        ),
        (&["ui/main.ts", "export.rs", "lib/util/mod.rs"], &[("web/app/main.ts", "rename-suffix")]),
        (
            &["src/billing", "src/", "mod.rs", "tools/x.py,"],
            &[
                ("src/a.rs", "dir"),
                ("src/b.rs", "dir"),
                ("src/billing/export.rs", "dir"),
                ("src/billing/mod.rs", "dir"),
                ("src/c.ts", "dir"),
                ("src/ledger/export.rs", "dir"),
                ("src/ledger/mod.rs", "dir"),
                ("tools/x.py", "exact"),
            ],
        ),
        (
            &["engine/src/intent/{paths,text}.rs", "README.md.", "_docs/guide.md_"],
            &[
                ("README.md", "exact"),
                ("docs/guide.md", "exact"),
                ("engine/src/intent/paths.rs", "glob"),
                ("engine/src/intent/text.rs", "glob"),
            ],
        ),
    ];
    for (t, want) in cases {
        assert_eq!(block(&r, t), want.to_vec(), "block {t:?}");
    }
    let line = r.report_line("toy", "test");
    assert!(line.starts_with("[id-paths] repo=toy surface=test candidates=16 renamed=3 resolved="), "{line}");
    assert!(line.contains(" routes=context:1,dir:9,dir-suffix:4,exact:7,glob:8,rename:1,rename-suffix:2 ambiguous=3 over_cap=0 unresolved=3"), "{line}");
    r.report("toy", "test");
}

#[test]
fn rename_pairs_oldest_first() {
    let commit = |c: &str, t: i64, files: &[(&str, Option<&str>)]| HistoryCommit {
        c: c.into(),
        t,
        files: files
            .iter()
            .map(|(p, from)| HistoryFile { p: p.to_string(), a: Some(1), d: Some(0), from: from.map(str::to_string) })
            .collect(),
    };
    let snap = HistorySnapshot {
        meta: HistoryMeta::new("abcdef0123456789".into(), 2000, None, String::new()),
        commits: vec![
            commit("c3", 3, &[("z.py", Some("y.py"))]),
            commit("c2", 2, &[("y.py", Some("x.py")), ("docs/x.md", None)]),
            commit("c1", 1, &[("x.py", None)]),
        ],
        blame: vec![],
    };
    let pairs = rename_pairs(&snap);
    assert_eq!(pairs, renames(&[("x.py", "y.py"), ("y.py", "z.py")]));
    let r = PathResolver::new(&cands(&["z.py"]), &pairs);
    assert_eq!(r.map("x.py"), Some(0));
}
