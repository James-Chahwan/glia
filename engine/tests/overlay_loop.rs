//! The overlay loop (CE.3b): a build with a given overlay text in place of
//! the primary repo's `.glia/overlay.toml` (`BuildOptions::with_overlay_text`),
//! and the format-preserving writer that merges a candidate's stanzas into the
//! user's file and takes one back out.
//!
//! The Go sources are the substrate-gap fixture `go-overlay-data-wrapper`, with
//! NewCollection's body changed to `.Collection(strings.ToLower(name))`: a
//! bare parameter handed to `.Collection` is a wrapper CA.4 infers without the
//! overlay, and the build with no overlay must mint no entity here.
//!
//! Pre-fix baseline (HEAD de96625): this file does not compile - BuildOptions
//! has no `with_overlay_text`, and `glia_engine::overlay_loop` exports nothing.

use std::path::{Path, PathBuf};

use glia_code_domain::glia_config::{self, LoadedConfig};
use glia_code_domain::{edge_category, node_kind};
use glia_engine::overlay_loop::{
    Location, StanzaRef, merge, parse_candidate, report_candidate, without,
};
use glia_engine::{BuildOptions, GenerateResult, generate_many_opts, generate_one_opts};

const CHAT_REPO_GO: &str = include_str!(
    "../../bench/substrate-gap/fixtures/go-overlay-data-wrapper/chat_preview_repository.go"
);
const COLLECTION_GO: &str =
    include_str!("../../bench/substrate-gap/fixtures/go-overlay-data-wrapper/collection.go");
const FIXTURE_OVERLAY: &str =
    include_str!("../../bench/substrate-gap/fixtures/go-overlay-data-wrapper/.glia/overlay.toml");
const ENTITY: &str = "data_entity:nosql:chat_previews";

/// A whole overlay text holding the fixture's `[[wrapper]]` stanza: what a
/// merge of that stanza into no file writes.
fn wrapper_text() -> String {
    let at = FIXTURE_OVERLAY
        .find("[[wrapper]]")
        .expect("the fixture overlay has a [[wrapper]]");
    format!("version = 1\n\n{}", &FIXTURE_OVERLAY[at..])
}

/// A fresh, empty temp dir for one test.
fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("glia_ce3b_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// The Go sources at `root`, NewCollection taking a computed name (no CA.4
/// inference); no overlay file.
fn go_repo(root: &Path) -> String {
    let collection = COLLECTION_GO
        .replace(
            "import (\n\t\"go.mongodb.org/mongo-driver/mongo\"\n)",
            "import (\n\t\"strings\"\n\n\t\"go.mongodb.org/mongo-driver/mongo\"\n)",
        )
        .replace(".Collection(name)", ".Collection(strings.ToLower(name))");
    assert_ne!(
        collection, COLLECTION_GO,
        "the fixture's NewCollection body moved"
    );
    assert!(
        collection.contains("\"strings\""),
        "the fixture's import block moved"
    );
    std::fs::create_dir_all(root).expect("mkdir");
    std::fs::write(root.join("collection.go"), collection).expect("write");
    std::fs::write(root.join("chat_preview_repository.go"), CHAT_REPO_GO).expect("write");
    root.to_str().expect("utf-8 temp path").to_string()
}

fn qname_of(r: &GenerateResult, id: glia_core::NodeId) -> String {
    r.merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// The repos (by root, as given) holding a DATA_ENTITY named [`ENTITY`].
fn entity_repos(r: &GenerateResult) -> Vec<String> {
    let mut out: Vec<String> = r
        .merged
        .graphs
        .iter()
        .filter(|g| {
            g.nodes.iter().any(|n| {
                g.nav.kind_by_id.get(&n.id) == Some(&node_kind::DATA_ENTITY)
                    && g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == ENTITY)
            })
        })
        .map(|g| r.repo_roots.get(&g.repo.0).cloned().unwrap_or_default())
        .collect();
    out.dedup();
    out
}

/// `(from, to)` of every ACCESSES_DATA edge into [`ENTITY`].
fn entity_accesses(r: &GenerateResult) -> Vec<(String, String)> {
    r.merged
        .all_edges()
        .filter(|e| e.category == edge_category::ACCESSES_DATA)
        .map(|e| (qname_of(r, e.from), qname_of(r, e.to)))
        .filter(|(_, to)| to == ENTITY)
        .collect()
}

/// Every node and edge of a build, as JSON: two builds of one tree compare equal.
fn graph_json(r: &GenerateResult) -> String {
    let nodes: Vec<_> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .collect();
    let edges: Vec<_> = r.merged.all_edges().collect();
    serde_json::to_string(&(nodes, edges)).expect("json")
}

#[test]
fn overlay_text_replaces_the_file_for_the_primary_repo() {
    let root = tmp("text");
    let repo = go_repo(&root.join("repo"));

    let plain = generate_one_opts(&repo, false, &BuildOptions::default()).expect("build");
    assert!(
        entity_repos(&plain).is_empty(),
        "no overlay, no inference: no entity"
    );
    assert!(entity_accesses(&plain).is_empty());

    let opts = BuildOptions::default().with_overlay_text(wrapper_text());
    assert_eq!(opts.overlay_text.as_deref(), Some(wrapper_text().as_str()));
    let with = generate_one_opts(&repo, false, &opts).expect("build");
    assert_eq!(
        entity_repos(&with),
        [repo.as_str()],
        "the text's [[wrapper]] minted the entity"
    );
    let access = entity_accesses(&with);
    assert_eq!(access.len(), 1, "{access:?}");
    assert!(
        access[0].0.ends_with("NewChatPreviewRepository"),
        "{access:?}"
    );
    assert!(
        !root.join("repo/.glia").exists(),
        "a text build writes no overlay file"
    );

    // A repo WITH the file, built with the text `version = 1`, is the same
    // repo with the file removed.
    std::fs::create_dir_all(root.join("repo/.glia")).expect("mkdir");
    std::fs::write(root.join("repo/.glia/overlay.toml"), FIXTURE_OVERLAY).expect("write");
    let from_file = generate_one_opts(&repo, false, &BuildOptions::default()).expect("build");
    assert_eq!(
        entity_repos(&from_file),
        [repo.as_str()],
        "the file's [[wrapper]] applies"
    );
    let empty = BuildOptions::default().with_overlay_text("version = 1\n".to_string());
    let text_over_file = generate_one_opts(&repo, false, &empty).expect("build");
    std::fs::remove_file(root.join("repo/.glia/overlay.toml")).expect("rm");
    let no_file = generate_one_opts(&repo, false, &BuildOptions::default()).expect("build");
    assert!(entity_repos(&text_over_file).is_empty());
    assert_eq!(graph_json(&text_over_file), graph_json(&no_file));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn overlay_text_applies_to_the_first_path_only() {
    let root = tmp("many");
    let first = go_repo(&root.join("first"));
    let second = go_repo(&root.join("second"));
    let repos = [first.clone(), second];
    let opts = BuildOptions::default().with_overlay_text(wrapper_text());
    let built = generate_many_opts(&repos, false, &opts).expect("build");
    assert_eq!(
        entity_repos(&built),
        [first.as_str()],
        "the second path keeps its own (absent) file"
    );
    std::fs::remove_dir_all(root).ok();
}

const BASE: &str = "\
# glia overlay for this repo
version = 1

# The HTTP client wrapper.
[[wrapper]]
call = \"request\"  # the fetch wrapper
kind = \"http\"
method_arg = 0
path_arg = 1

# A hand-declared call.
[[edge]]
from = \"a::caller\"
to = \"b::callee\"
category = \"CALLS\"

# Pinned constants.
[constants]
API_BASE = \"/api\"

# end of file
";

const CANDIDATE: &str = "\
# gap: gap:0123456789abcdef
[[edge]]
from = \"web::client::load\"
to = \"GET /users\"
category = \"HTTP_CALLS\"

[constants]
GATEWAY = \"/gw\"

[entrypoints]
qnames = [\"nightly::Nightly\"]
";

fn counts(l: &LoadedConfig) -> Vec<(&'static str, usize)> {
    l.section_counts().to_vec()
}

fn count(l: &LoadedConfig, section: &str) -> usize {
    l.section_counts()
        .iter()
        .find(|(s, _)| *s == section)
        .map_or(0, |(_, n)| *n)
}

/// `sub`'s lines appear in `all`, in order (only lines were inserted).
fn is_subsequence(sub: &str, all: &str) -> bool {
    let mut it = all.lines();
    sub.lines().all(|l| it.any(|a| a == l))
}

#[test]
fn merge_preserves_the_user_file() {
    let cand = report_candidate(Some("candidate.toml"), CANDIDATE).expect("a valid candidate");
    let shown: Vec<String> = cand.stanzas().iter().map(StanzaRef::to_string).collect();
    assert_eq!(shown, ["edge#1", "constants.GATEWAY", "entrypoints#1"]);
    let edge = &cand.stanzas()[0];
    assert_eq!(edge.gaps, ["gap:0123456789abcdef"]);
    assert!(cand.stanzas()[1].gaps.is_empty());
    assert_eq!(
        cand.marker(Some("candidate.toml")),
        "[overlay] candidate file=candidate.toml stanzas=3 (route_prefix=0 wrapper=0 edge=1 constants=1 entrypoints=1) gap_links=1 errors=0"
    );

    let merged = merge(Some(BASE), &cand, None).expect("merge");
    let text = &merged.text;
    assert!(
        is_subsequence(BASE, text),
        "every base line, byte for byte and in order:\n{text}"
    );
    assert_eq!(text.lines().count(), BASE.lines().count() + 12, "{text}");
    // The edge lands right after the last [[edge]], its gap link above it.
    let expect_edge = "category = \"CALLS\"\n\n# gap: gap:0123456789abcdef\n[[edge]]\nfrom = \"web::client::load\"\nto = \"GET /users\"\ncategory = \"HTTP_CALLS\"\n\n# Pinned constants.\n";
    assert!(text.contains(expect_edge), "{text}");
    // The constant joins the existing [constants]; the new [entrypoints] goes at the end.
    assert!(
        text.contains("[constants]\nAPI_BASE = \"/api\"\nGATEWAY = \"/gw\"\n"),
        "{text}"
    );
    assert!(
        text.ends_with(
            "[entrypoints]\nqnames = [\n    \"nightly::Nightly\",\n]\n\n# end of file\n"
        ),
        "{text}"
    );
    assert_eq!(merged.duplicates, 0);
    let at: Vec<Location> = merged.refs.iter().map(|(_, l)| l.clone()).collect();
    assert_eq!(
        at,
        [
            Location::Table {
                section: "edge",
                index: 1
            },
            Location::Constant {
                name: "GATEWAY".into()
            },
            Location::Entrypoint {
                index: 0,
                pattern: "nightly::Nightly".into()
            },
        ]
    );

    let base = glia_config::parse_str(BASE);
    let after = glia_config::parse_str(text);
    assert!(base.errors.is_empty(), "{:?}", base.errors);
    assert!(after.errors.is_empty(), "{:?}", after.errors);
    for ((section, b), (_, a)) in counts(&base).into_iter().zip(counts(&after)) {
        let added = usize::from(matches!(section, "edge" | "constants" | "entrypoints"));
        assert_eq!(a, b + added, "{section}");
    }

    // No file: the merge starts from `version = 1`.
    let fresh = merge(None, &cand, None).expect("merge");
    assert!(fresh.text.starts_with("version = 1\n"), "{}", fresh.text);
    assert!(glia_config::parse_str(&fresh.text).errors.is_empty());

    // `only` picks stanzas by ref; a ref the candidate lacks is an error.
    let only = merge(Some(BASE), &cand, Some(&cand.stanzas()[1..2])).expect("merge");
    assert_eq!(only.refs.len(), 1);
    assert_eq!(count(&glia_config::parse_str(&only.text), "edge"), 1);
    assert_eq!(count(&glia_config::parse_str(&only.text), "constants"), 2);
    let other = parse_candidate(
        "[[wrapper]]\ncall = \"x\"\nkind = \"http\"\nmethod = \"GET\"\npath_arg = 0\n",
    )
    .expect("valid");
    let err = merge(Some(BASE), &cand, Some(other.stanzas()))
        .expect_err("wrapper#1 is not in the candidate");
    assert!(err.contains("wrapper#1") && err.contains("edge#1"), "{err}");
}

#[test]
fn conflicting_constant_is_refused() {
    let base = "version = 1\n\n[constants]\nGATEWAY = \"/gw\"\n";
    let cand = parse_candidate("[constants]\nGATEWAY = \"/other\"\n").expect("valid");
    let err = merge(Some(base), &cand, None).expect_err("a pinned constant is never re-pinned");
    assert_eq!(err, "constant GATEWAY already pinned to \"/gw\"");

    // The same value is a duplicate: nothing written, the text unchanged.
    let same =
        parse_candidate("[constants]\nGATEWAY = \"/gw\"\n\n[entrypoints]\nqnames = [\"a::b\"]\n")
            .expect("valid");
    let with_ep =
        "version = 1\n\n[constants]\nGATEWAY = \"/gw\"\n\n[entrypoints]\nqnames = [\"a::b\"]\n";
    let merged = merge(Some(with_ep), &same, None).expect("merge");
    assert_eq!(merged.duplicates, 2);
    assert_eq!(merged.text, with_ep);
    assert!(
        merged
            .refs
            .iter()
            .all(|(_, at)| *at == Location::AlreadyPresent)
    );
    for (r, _) in &merged.refs {
        assert_eq!(merged.without(r).expect("without"), with_ep, "{r}");
    }
}

#[test]
fn without_removes_exactly_one() {
    let cand = parse_candidate(CANDIDATE).expect("valid");
    let merged = merge(Some(BASE), &cand, None).expect("merge");
    let all = glia_config::parse_str(&merged.text);
    assert_eq!(merged.refs.len(), 3);
    for (r, at) in &merged.refs {
        let out = without(&merged.text, at).expect("without");
        assert_eq!(merged.without(r).expect("without by ref"), out, "{r}");
        let loaded = glia_config::parse_str(&out);
        assert!(loaded.errors.is_empty(), "{r}: {:?}", loaded.errors);
        for ((section, n), (_, m)) in counts(&all).into_iter().zip(counts(&loaded)) {
            let removed = usize::from(section == r.section);
            assert_eq!(m, n - removed, "{r}: {section}");
        }
        assert!(
            is_subsequence(&out, &merged.text),
            "{r}: only lines were removed:\n{out}"
        );
        assert!(
            is_subsequence(BASE, &out),
            "{r}: every base line stays:\n{out}"
        );
        // The other two stanzas are still there, byte for byte.
        for (other, _) in merged.refs.iter().filter(|(o, _)| o != r) {
            let line = match other.section {
                "edge" => "from = \"web::client::load\"",
                "constants" => "GATEWAY = \"/gw\"",
                _ => "    \"nightly::Nightly\",",
            };
            assert!(
                out.lines().any(|l| l == line),
                "{r} removed {other}:\n{out}"
            );
        }
    }
    // Taking the edge back out is exactly the merge of the other two.
    let others: Vec<StanzaRef> = cand.stanzas()[1..].to_vec();
    let edge_out = without(&merged.text, &merged.refs[0].1).expect("without");
    assert!(
        !edge_out.contains("gap:0123456789abcdef"),
        "the gap link goes with its stanza"
    );
    assert_eq!(
        edge_out,
        merge(Some(BASE), &cand, Some(&others)).expect("merge").text
    );
    // A location the text does not hold is an error, never a guess.
    let stale = Location::Entrypoint {
        index: 0,
        pattern: "other::Pattern".into(),
    };
    assert!(without(&merged.text, &stale).is_err());
    let past = Location::Table {
        section: "edge",
        index: 9,
    };
    assert!(without(&merged.text, &past).is_err());
}

#[test]
fn candidate_sections_are_limited() {
    let walk = parse_candidate("[walk]\nskip = [\"gen/\"]\n").expect_err("[walk] is user config");
    assert!(walk.contains("[walk]"), "{walk}");
    assert!(
        walk.contains("a candidate carries only overlay sections and entrypoints"),
        "{walk}"
    );
    let rule = parse_candidate("[[constraint]]\nid = \"r\"\nkind = \"invariant\"\ntext = \"t\"\n")
        .expect_err("[[constraint]] is declared knowledge");
    assert!(rule.contains("[[constraint]]"), "{rule}");

    // An unknown key: the loader's own error, quoted, at the candidate's line.
    let bad = parse_candidate(
        "[[wrapper]]\ncall = \"x\"\nkind = \"http\"\nmethod = \"GET\"\npath_arg = 0\nbogus = 1\n",
    )
    .expect_err("the loader rejects the key");
    assert!(bad.contains("unknown field `bogus`"), "{bad}");
    assert!(bad.starts_with("candidate:"), "{bad}");
    // A stanza the loader would drop refuses the whole candidate.
    let dropped = parse_candidate("[[route_prefix]]\nscope = \".\"\nprefix = \"orders\"\n")
        .expect_err("the loader drops the stanza");
    assert!(
        dropped.contains("candidate:1: [[route_prefix]] prefix \"orders\""),
        "{dropped}"
    );
    // A gap link must be an id.
    let link = parse_candidate(
        "# gap: orders\n[[edge]]\nfrom = \"a\"\nto = \"b\"\ncategory = \"CALLS\"\n",
    )
    .expect_err("not a gap id");
    assert!(
        link.contains("edge#1") && link.contains("\"orders\""),
        "{link}"
    );
    // `version = 1` is optional; any other version is refused.
    assert!(parse_candidate("version = 1\n[constants]\nA = \"a\"\n").is_ok());
    assert!(parse_candidate("version = 2\n[constants]\nA = \"a\"\n").is_err());
    // Several gap links on one stanza, and one pattern's own link inside a
    // multi-line array.
    let many = parse_candidate(
        "# gap: gap:0000000000000001\n# note: two gaps\n# gap: gap:0000000000000002\n[[wrapper]]\ncall = \"x\"\nkind = \"http\"\nmethod = \"GET\"\npath_arg = 0\n\n[entrypoints]\nqnames = [\n  \"a::b\",\n  # gap: gap:0000000000000003\n  \"c::d\",\n]\n",
    )
    .expect("valid");
    let gaps: Vec<Vec<String>> = many.stanzas().iter().map(|s| s.gaps.clone()).collect();
    assert_eq!(
        gaps,
        [
            vec![
                "gap:0000000000000001".to_string(),
                "gap:0000000000000002".to_string()
            ],
            vec![],
            vec!["gap:0000000000000003".to_string()],
        ]
    );
}
