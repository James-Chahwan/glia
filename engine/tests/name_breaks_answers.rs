//! FH.7b acceptance: FH.7a's style-agnostic name breaks inside the two PR
//! answers. `contract_breaks_vs_rev` lists them as `names` (advisory by
//! default, blocking only under `models_min_tier` `heuristic`), and
//! `review_vs_rev` as `name_breaks` with a `### Name breaks (advisory)`
//! section. The change is FH.7a's `drift-rename-go-ts` fixture: BASE
//! committed, the broken state (the server's json tag renamed, the TS client
//! still reading the old name) in the working tree, and the fixed state
//! (the client follows) clearing every row.
//!
//! fired_on, once per answer, after FH.7a's own `[name-breaks] base=..`:
//! `[name-breaks] answer rows=<k> blocking=<b> advisory=<a> min_tier=<t>
//! folded_schema_rows=<s>-><t> surface=<contract_breaks|review>`.

use crate::git_fixture;

use std::fs;
use std::path::{Path, PathBuf};

use git_fixture::GitRepo;
use glia_engine::contract_breaks::{ContractBreakArgs, NAME_MIN_TIERS, contract_breaks_vs_rev};
use glia_engine::review::{MarkdownOptions, ReviewArgs, render_markdown, review_vs_rev};

const FIXTURE: &str = "drift-rename-go-ts";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(FIXTURE)
}

/// Every file under `dir`, `/`-separated and relative to it, sorted.
fn files_under(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, at: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<_> = fs::read_dir(at)
            .unwrap_or_else(|e| panic!("read {}: {e}", at.display()))
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).expect("under root");
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

fn write_broken(repo: &GitRepo) {
    let dir = fixture_dir();
    for rel in files_under(&dir) {
        if rel == "key.json" || rel.starts_with("drift/") {
            continue;
        }
        repo.write(
            &rel,
            &fs::read_to_string(dir.join(&rel)).expect("fixture file"),
        );
    }
}

fn write_state(repo: &GitRepo, state: &str) {
    let dir = fixture_dir().join("drift").join(state);
    for rel in files_under(&dir) {
        let text = fs::read_to_string(dir.join(&rel)).expect("drift file");
        repo.write(rel.strip_suffix(".drift").expect(".drift suffix"), &text);
    }
}

/// BASE committed, the broken state in the working tree.
fn broken() -> GitRepo {
    let repo = GitRepo::init();
    write_broken(&repo);
    write_state(&repo, "base");
    repo.commit("base");
    write_broken(&repo);
    repo
}

fn args(min_tier: &'static str) -> ContractBreakArgs {
    let mut a = ContractBreakArgs::default();
    a.models_min_tier = min_tier;
    a
}

#[test]
fn contract_breaks_lists_the_stale_client_sites_as_advisory_names() {
    let repo = broken();
    let b = contract_breaks_vs_rev(repo.path(), "HEAD", &args("fact")).expect("contract breaks");
    let sites: Vec<(&str, &str, i64, &str)> = b
        .names
        .iter()
        .map(|r| (r.name.as_str(), r.file.as_str(), r.line, r.position))
        .collect();
    assert_eq!(
        sites,
        vec![
            ("messageId", "web/src/models/message.ts", 2, "key"),
            ("messageId", "web/src/views/thread.ts", 5, "property"),
        ],
        "{b:#?}"
    );
    assert!(b.names.iter().all(|r| !r.blocking), "{b:#?}");
    // The fixture's tag rename is an http_model break too (DT.1o): one
    // advisory model row beside the two name rows.
    let models = b.models.iter().filter(|m| m.status == "breaking").count();
    assert_eq!(models, 1, "{b:#?}");
    assert_eq!(b.advisory, models + 2, "both name rows advisory: {b:#?}");
    assert_eq!(b.breaking, 0, "a name row never fails the default gate");
    assert!(b.absence.is_none(), "a name row is a row: {b:#?}");
    let json = serde_json::to_value(&b).expect("json");
    assert_eq!(json["names"].as_array().map(Vec::len), Some(2), "{json}");
}

#[test]
fn contract_breaks_blocks_on_names_only_under_heuristic() {
    assert!(NAME_MIN_TIERS.contains(&"heuristic"));
    let repo = broken();
    let derived =
        contract_breaks_vs_rev(repo.path(), "HEAD", &args("derived")).expect("contract breaks");
    // Under derived the model row blocks, the name rows stay advisory.
    let blocking_models = derived.models.iter().filter(|m| m.blocking).count();
    assert_eq!(blocking_models, 1, "{derived:#?}");
    assert_eq!(derived.breaking, blocking_models, "{derived:#?}");
    assert_eq!(derived.advisory, 2, "{derived:#?}");
    // Under heuristic the name rows block too; the model row is judged as
    // under derived.
    let heuristic =
        contract_breaks_vs_rev(repo.path(), "HEAD", &args("heuristic")).expect("contract breaks");
    assert_eq!(heuristic.breaking, blocking_models + 2, "{heuristic:#?}");
    assert_eq!(heuristic.advisory, 0, "{heuristic:#?}");
    assert!(heuristic.names.iter().all(|r| r.blocking), "{heuristic:#?}");
    let err = contract_breaks_vs_rev(repo.path(), "HEAD", &args("bogus")).expect_err("bad tier");
    assert!(err.contains("fact, derived, heuristic"), "{err}");
}

#[test]
fn the_fixed_state_lists_no_name_row() {
    let repo = broken();
    write_state(&repo, "fixed");
    let b = contract_breaks_vs_rev(repo.path(), "HEAD", &args("heuristic")).expect("cb");
    assert!(b.names.is_empty(), "{b:#?}");
    assert_eq!(b.breaking, 0, "{b:#?}");
    let json = serde_json::to_value(&b).expect("json");
    assert!(
        json.get("names").is_none(),
        "empty names are not serialised: {json}"
    );
}

#[test]
fn review_carries_the_name_breaks_advisory_section() {
    let repo = broken();
    let r = review_vs_rev(repo.path(), "HEAD", &ReviewArgs::default()).expect("review");
    let files: Vec<&str> = r.name_breaks.iter().map(|n| n.file.as_str()).collect();
    assert_eq!(
        files,
        vec!["web/src/models/message.ts", "web/src/views/thread.ts"],
        "{:#?}",
        r.name_breaks
    );
    assert_eq!(r.counts.name_breaks, 2);
    assert!(!r.blocking, "review's name breaks never block");
    let md = render_markdown(&r, &MarkdownOptions::default());
    assert!(md.contains("### Name breaks (advisory)"), "{md}");
    assert!(md.contains("`messageId` -> `messageUid`"), "{md}");
    assert!(md.contains("web/src/views/thread.ts:5"), "{md}");

    write_state(&repo, "fixed");
    let fixed = review_vs_rev(repo.path(), "HEAD", &ReviewArgs::default()).expect("review");
    assert!(fixed.name_breaks.is_empty(), "{:#?}", fixed.name_breaks);
    let json = serde_json::to_value(&fixed).expect("json");
    assert!(json.get("name_breaks").is_none(), "{json}");
    assert!(json["counts"].get("name_breaks").is_none(), "{json}");
    let md = render_markdown(&fixed, &MarkdownOptions::default());
    assert!(!md.contains("### Name breaks"), "{md}");
}
