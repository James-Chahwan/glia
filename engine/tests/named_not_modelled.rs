//! FB.13c — named but not modelled.
//!
//! find's `no_match` absence names where an identifier of the query occurs
//! (`named_in`, read off the vocabulary snapshot, FACT), and gaps'
//! `named_not_modelled` lists the identifiers a markdown doc cites in inline
//! code that occur in a tracked code file but that no node names.
//!
//! The snapshot is written through `glia_code_domain::snapshots::write_vocab`
//! with the tokens `vocab_tokens` reads (what `glia vocab sync` writes for a
//! committed tree): the engine's tests never depend on the process-spawning
//! snapshots crate. `cli/tests/gaps_cli.rs` / `find_cli.rs` run the real
//! `glia vocab sync` over a git repo.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glia_code_domain::snapshots::{VocabFile, VocabMeta, VocabToken, vocab_tokens, write_vocab};
use glia_engine::find::{FindOptions, find_nodes};
use glia_engine::gaps::{GapsOptions, NAMED_NOT_MODELLED, gaps_report};
use glia_engine::{GenerateResult, generate_one};

const README: &str = "# Service\n\nThe client reads `retryBudget` from its config and calls `parseTimeout()`.\nIt waits for `the` reply and for `HTTP` headers.\n\n## Limits\n\nSee `config.retryBudget` and `docOnlyName`.\n";
const CONFIG_TS: &str = "export const config = { retryBudget: 3 };\n\nexport function parseTimeout(ms: number) {\n  return ms * config.retryBudget;\n}\n";

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("glia_fb13c_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
}

/// Every file under `root` (no `.glia/`), repo-relative, in path order.
fn files_of(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).expect("read_dir").flatten() {
            let p = e.path();
            if p.file_name().is_some_and(|n| n == ".glia") {
                continue;
            }
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).expect("under root");
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// The vocabulary snapshot of the files under `root`, as `vocab sync` writes it.
fn sync_vocab(root: &Path) {
    let paths = files_of(root);
    let mut by_token: BTreeMap<String, (u64, Vec<u32>)> = BTreeMap::new();
    for (f, path) in paths.iter().enumerate() {
        let bytes = std::fs::read(root.join(path)).expect("read");
        for (t, n) in vocab_tokens(&bytes) {
            let e = by_token.entry(t).or_default();
            e.0 += u64::from(n);
            e.1.push(f as u32);
        }
    }
    let files: Vec<VocabFile> = paths
        .iter()
        .enumerate()
        .map(|(f, p)| VocabFile::new(f as u32, p.clone()))
        .collect();
    let tokens: Vec<VocabToken> = by_token
        .into_iter()
        .map(|(t, (n, ids))| VocabToken::new(t, n, ids))
        .collect();
    write_vocab(root, VocabMeta::new("0".repeat(40), 0, 0), &files, &tokens).expect("vocab");
}

fn fixture(tag: &str, vocab: bool) -> (PathBuf, GenerateResult) {
    let root = tmp(tag);
    write(&root, "README.md", README);
    write(&root, "src/config.ts", CONFIG_TS);
    if vocab {
        sync_vocab(&root);
    }
    let r = generate_one(root.to_str().expect("utf-8")).expect("build");
    (root, r)
}

fn roots_of(r: &GenerateResult) -> Vec<(u64, PathBuf)> {
    r.repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect()
}

fn vocab_roots(r: &GenerateResult) -> BTreeMap<u64, PathBuf> {
    roots_of(r).into_iter().collect()
}

#[test]
fn gaps_lists_a_cited_identifier_no_node_names() {
    let (root, r) = fixture("gaps", true);
    let rep = gaps_report(&r.merged, &roots_of(&r), &GapsOptions::default()).expect("report");
    std::fs::remove_dir_all(&root).ok();
    let rows: Vec<_> = rep
        .rows
        .iter()
        .filter(|g| g.category == NAMED_NOT_MODELLED)
        .collect();
    // parseTimeout is a FUNCTION (a node), `the` / `HTTP` are prose, and
    // docOnlyName occurs in no code file: retryBudget alone.
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rep.count(NAMED_NOT_MODELLED), 1);
    let row = rows[0];
    assert_eq!(row.qname, "retryBudget");
    assert_eq!(row.kind, "identifier");
    assert_eq!(row.file.as_deref(), Some("README.md"));
    assert_eq!(row.line, Some(3));
    assert_eq!(row.tier, "fact");
    assert!(row.id.starts_with("gap:"), "{}", row.id);
    assert!(row.detail.starts_with("repo="), "{}", row.detail);
    assert!(
        row.detail.ends_with(
            " cited by 2 doc section(s); occurs in 1 code file(s) (src/config.ts); no node named retryBudget"
        ),
        "{}",
        row.detail
    );
    assert!(
        row.suggest
            .starts_with("check the parser for typescript covers"),
        "{}",
        row.suggest
    );
}

#[test]
fn gaps_without_a_vocab_snapshot_has_no_rows() {
    let (root, r) = fixture("gaps-none", false);
    let rep = gaps_report(&r.merged, &roots_of(&r), &GapsOptions::default()).expect("report");
    std::fs::remove_dir_all(&root).ok();
    assert_eq!(rep.count(NAMED_NOT_MODELLED), 0);
    assert!(rep.rows.iter().all(|g| g.category != NAMED_NOT_MODELLED));
    // No roots: a root category, so it is skipped.
    let bare = gaps_report(&r.merged, &[], &GapsOptions::default()).expect("report");
    assert!(
        bare.skipped.contains(&NAMED_NOT_MODELLED),
        "{:?}",
        bare.skipped
    );
}

#[test]
fn find_absence_names_where_the_identifier_occurs() {
    let (root, r) = fixture("find", true);
    let mut opts = FindOptions::default();
    opts.vocab_roots = Some(vocab_roots(&r));
    let answer = find_nodes(&r.merged, "retryBudget", &opts);
    std::fs::remove_dir_all(&root).ok();
    let a = answer.absence.expect("no node named retryBudget");
    assert_eq!(a.reason, "no_match");
    assert_eq!(a.tier, "fact");
    assert_eq!(a.named_in.len(), 1, "{:?}", a.named_in);
    let n = &a.named_in[0];
    assert_eq!(n.token, "retryBudget");
    assert_eq!(n.files, ["README.md", "src/config.ts"]);
    assert_eq!(n.file_count, 2);
    assert_eq!(n.occurrences, 4);
    assert!(!n.common);
    assert_eq!(
        a.note,
        "no node matches `retryBudget` by name or qname in any find tier; `retryBudget` occurs in 2 tracked file(s) (README.md, src/config.ts) but no node is named retryBudget"
    );
    let json = serde_json::to_value(&a).expect("json");
    assert_eq!(json["named_in"][0]["token"], "retryBudget");
}

#[test]
fn find_absence_skips_a_token_a_node_names() {
    // `parseTimeout` names a FUNCTION: a two-token query that matches no node
    // names only the token no node is named.
    let (root, r) = fixture("find-two", true);
    let mut opts = FindOptions::default();
    opts.vocab_roots = Some(vocab_roots(&r));
    let answer = find_nodes(&r.merged, "parseTimeout retryBudget", &opts);
    std::fs::remove_dir_all(&root).ok();
    let a = answer.absence.expect("no match");
    let tokens: Vec<&str> = a.named_in.iter().map(|n| n.token.as_str()).collect();
    assert_eq!(tokens, ["retryBudget"]);
}

#[test]
fn find_absence_without_a_snapshot_is_unchanged() {
    let (root, r) = fixture("find-none", false);
    let plain = find_nodes(&r.merged, "retryBudget", &FindOptions::default());
    let mut opts = FindOptions::default();
    opts.vocab_roots = Some(vocab_roots(&r));
    let with_roots = find_nodes(&r.merged, "retryBudget", &opts);
    std::fs::remove_dir_all(&root).ok();
    let a = serde_json::to_string(&plain).expect("json");
    let b = serde_json::to_string(&with_roots).expect("json");
    assert_eq!(a, b);
    assert!(!a.contains("named_in"), "{a}");
    let note = plain.absence.expect("absence").note;
    assert_eq!(
        note,
        "no node matches `retryBudget` by name or qname in any find tier"
    );
}
