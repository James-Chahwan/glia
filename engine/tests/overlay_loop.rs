//! The overlay loop (CE.3b): a build with a given overlay text in place of
//! the primary repo's `.glia/overlay.toml` (`BuildOptions::with_overlay_text`),
//! and the format-preserving writer that merges a candidate's stanzas into the
//! user's file and takes one back out. The trial (CE.3c): `try_candidate`
//! builds base, base + candidate and each leave-one-out on one parse cache,
//! and attributes every effect to its stanza.
//!
//! The Go sources are the substrate-gap fixture `go-overlay-data-wrapper`, with
//! NewCollection's body changed to `.Collection(strings.ToLower(name))`: a
//! bare parameter handed to `.Collection` is a wrapper CA.4 infers without the
//! overlay, and the build with no overlay must mint no entity here.
//!
//! Pre-fix baseline (HEAD de96625): this file does not compile - BuildOptions
//! has no `with_overlay_text`, and `glia_engine::overlay_loop` exports nothing.
//! CE.3c's baseline (HEAD 1da2539): the trial tests do not compile -
//! `glia_engine::overlay_loop` has no `try_candidate`; `gaps::overlay_delta`
//! judges the whole file only, so no stanza's effect is attributed.
//!
//! Propose and accept (CE.3d): `propose` is the gap work list with each
//! row's source snippet, `accept` the only writer of `.glia/overlay.toml`
//! (chosen candidate stanzas in, orphaned / redundant rules out by gap id,
//! validated by the loader, written atomically). CE.3d's baseline (HEAD
//! 7425636): these tests do not compile - `glia_engine::overlay_loop` has no
//! `propose` / `accept`, and `glia overlay propose /tmp` is
//! `error: unrecognized subcommand 'overlay'`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::glia_config::{self, LoadedConfig};
use glia_code_domain::{edge_category, node_kind};
use glia_engine::gaps::{
    DEAD_SYMBOL, DROP, GapRow, GapsOptions, GapsReport, KEEP, ORPHANED_RULE, REDUNDANT_RULE,
    UNPAIRED_ROUTE, UNRESOLVED_ENDPOINT, WRAPPED_SINK, gaps_report,
};
use glia_engine::overlay_loop::{
    AcceptOptions, Location, Proposal, ProposeOptions, StanzaRef, StanzaTrial, TryOptions,
    TryReport, accept, merge, parse_candidate, propose, report_candidate, try_candidate, without,
};
use glia_engine::persist::{default_layout_dir, persist_result};
use glia_engine::{
    BuildOptions, GenerateResult, generate_many_opts, generate_one, generate_one_incremental,
    generate_one_opts,
};

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

// ---------------------------------------------------------------------------
// The trial (CE.3c)
// ---------------------------------------------------------------------------

/// The fixture's `[[wrapper]]` stanza, the table alone.
fn wrapper_stanza() -> &'static str {
    let at = FIXTURE_OVERLAY
        .find("[[wrapper]]")
        .expect("the fixture overlay has a [[wrapper]]");
    &FIXTURE_OVERLAY[at..]
}

/// A useful pattern and a useless edge (neither qname exists).
const ENTRYPOINT_AND_EDGE: &str = "\
[entrypoints]
qnames = [\"nightly::Nightly\"]

[[edge]]
from = \"nowhere::Caller\"
to = \"nowhere::Callee\"
category = \"CALLS\"
";

/// Three stanzas: the wrapper (mints the entity), the pattern (makes
/// `Nightly` live) and the edge (binds nothing).
fn three_stanzas() -> String {
    format!("{}\n{ENTRYPOINT_AND_EDGE}", wrapper_stanza())
}

/// [`go_repo`] plus `nightly.go`, a function nothing calls; no overlay file.
fn nightly_repo(root: &Path) -> String {
    let repo = go_repo(root);
    std::fs::write(
        root.join("nightly.go"),
        "package repositories\n\nfunc Nightly() {}\n",
    )
    .expect("write");
    repo
}

fn stanza<'r>(report: &'r TryReport, name: &str) -> &'r StanzaTrial {
    report
        .stanzas
        .iter()
        .find(|s| s.stanza == name)
        .unwrap_or_else(|| panic!("no stanza {name} in {report:#?}"))
}

fn map(entries: &[(&'static str, i64)]) -> BTreeMap<&'static str, i64> {
    entries.iter().copied().collect()
}

#[test]
fn try_attributes_each_stanza() {
    let root = tmp("try");
    let repo = nightly_repo(&root.join("repo"));
    let report = try_candidate(
        std::slice::from_ref(&repo),
        &three_stanzas(),
        &TryOptions::default(),
    )
    .expect("try");

    let names: Vec<&str> = report.stanzas.iter().map(|s| s.stanza.as_str()).collect();
    assert_eq!(
        names,
        ["wrapper#1", "entrypoints#1", "edge#1"],
        "candidate order"
    );
    assert_eq!(report.builds, 5, "base, with, three leave-one-out");

    let wrapper = stanza(&report, "wrapper#1");
    assert_eq!(
        wrapper.marginal.nodes,
        map(&[("DATA_ENTITY", 1)]),
        "{wrapper:#?}"
    );
    assert_eq!(
        wrapper.marginal.edges,
        map(&[("ACCESSES_DATA", 1)]),
        "{wrapper:#?}"
    );
    assert!(wrapper.marginal.gaps.is_empty(), "{wrapper:#?}");
    assert_eq!(wrapper.verdict, KEEP);

    let pattern = stanza(&report, "entrypoints#1");
    assert_eq!(
        pattern.marginal.gaps,
        map(&[("dead_symbol", -1)]),
        "{pattern:#?}"
    );
    assert!(pattern.marginal.nodes.is_empty() && pattern.marginal.edges.is_empty());
    assert_eq!(pattern.verdict, KEEP);

    let edge = stanza(&report, "edge#1");
    assert!(edge.marginal.is_empty(), "{edge:#?}");
    assert_eq!(edge.verdict, DROP);

    assert_eq!(report.verdict, KEEP);
    assert_eq!(report.base.gaps_by_category.get("dead_symbol"), Some(&4));
    assert_eq!(report.with.gaps_by_category.get("dead_symbol"), Some(&3));
    assert_eq!((report.base.total_gaps(), report.with.total_gaps()), (4, 3));
    assert_eq!(
        report.delta.nodes,
        map(&[("DATA_ENTITY", 1)]),
        "the totals are the sum here: no stanza needs another"
    );
    assert!(report.closed.is_empty(), "no stanza links a gap");
    assert!(
        report
            .stanzas
            .iter()
            .all(|s| s.gaps.is_empty() && s.closes.is_empty())
    );

    // Without leave-one-out: two builds, the same totals, no attribution.
    let mut two = TryOptions::default();
    two.leave_one_out = false;
    let quick = try_candidate(std::slice::from_ref(&repo), &three_stanzas(), &two).expect("try");
    assert_eq!(quick.builds, 2);
    assert!(quick.stanzas.is_empty());
    assert_eq!(
        (&quick.base, &quick.with, quick.verdict),
        (&report.base, &report.with, report.verdict)
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn closes_its_target_gap() {
    let root = tmp("closes");
    let web = root.join("web");
    let api = root.join("api");
    std::fs::create_dir_all(web.join("src")).expect("mkdir");
    std::fs::create_dir_all(&api).expect("mkdir");
    std::fs::write(
        web.join("src/client.ts"),
        "export function request(method: string, path: string) {\n  return fetch(path, { method });\n}\n\nexport async function loadUsers() {\n  return request('GET', '/users');\n}\n",
    )
    .expect("write");
    std::fs::write(
        api.join("app.py"),
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef list_users():\n    return []\n\n\n@app.route(\"/orders\", methods=[\"GET\"])\ndef list_orders():\n    return []\n",
    )
    .expect("write");
    let repos = [
        web.to_str().expect("utf-8").to_string(),
        api.to_str().expect("utf-8").to_string(),
    ];

    // The target: the one `<unresolved>` sink of the build as it is.
    let plain = generate_many_opts(&repos, false, &BuildOptions::default()).expect("build");
    let gaps = gaps_report(&plain.merged, &[], &GapsOptions::default()).expect("gaps");
    let target: Vec<&str> = gaps
        .rows
        .iter()
        .filter(|r| r.category == UNRESOLVED_ENDPOINT)
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(target.len(), 1, "{:#?}", gaps.rows);
    let id = target[0].to_string();
    let want = vec![id.clone()];

    let cand = format!(
        "# gap: {id}\n[[edge]]\nfrom = \"endpoint:GET:<unresolved>\"\nto = \"GET /users\"\ncategory = \"HTTP_CALLS\"\n"
    );
    let report = try_candidate(&repos, &cand, &TryOptions::default()).expect("try");
    let edge = stanza(&report, "edge#1");
    assert_eq!(edge.gaps, want);
    assert_eq!(edge.closes, want, "{edge:#?}");
    assert_eq!(report.closed, want);
    assert_eq!(
        edge.marginal.gaps,
        map(&[("unpaired_route", -1), ("unresolved_endpoint", -1)]),
        "{edge:#?}"
    );
    assert_eq!(edge.marginal.edges, map(&[("HTTP_CALLS", 1)]), "{edge:#?}");
    assert_eq!(edge.verdict, KEEP);
    assert_eq!(report.verdict, KEEP);
    assert_eq!((report.base.total_gaps(), report.with.total_gaps()), (4, 2));
    // One stanza: WITHOUT is BASE's overlay again, so its build is reused.
    assert_eq!(report.builds, 2);
    std::fs::remove_dir_all(root).ok();
}

const CHILD_ENV: &str = "GLIA_CE3C_CHILD_TRY";

/// The child half of [`try_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it tries [`three_stanzas`] on that repo so
/// its stderr (with `--nocapture`) carries the markers.
#[test]
fn child_try_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        try_candidate(&[dir], &three_stanzas(), &TryOptions::default()).expect("try");
    }
}

/// The stderr lines of one [`child_try_for_stderr`] over `dir`.
fn try_stderr(dir: &str) -> Vec<String> {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "--exact",
            "child_try_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, dir)
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "child try failed: {stderr}");
    stderr.lines().map(String::from).collect()
}

#[test]
fn parses_once() {
    let root = tmp("once");
    let repo = nightly_repo(&root.join("repo"));
    // A prior build writes the sidecar the try loads.
    generate_one_incremental(&repo).expect("build");

    let lines = try_stderr(&repo);
    let parses: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with(&format!("[incremental] {repo}: reused")))
        .collect();
    assert_eq!(parses.len(), 5, "one parse step per build: {lines:#?}");
    for l in &parses {
        assert!(l.contains(", reparsed 0, evicted 0"), "{l}");
    }
    let saves: Vec<&String> = lines
        .iter()
        .filter(|l| {
            l.starts_with("[incremental] unchanged") || l.starts_with("[incremental] saved")
        })
        .collect();
    assert_eq!(saves.len(), 1, "the cache is saved once: {lines:#?}");
    assert!(
        saves[0].starts_with("[incremental] unchanged"),
        "{}",
        saves[0]
    );
    let marker: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("[overlay] try "))
        .collect();
    assert_eq!(
        marker,
        [&format!(
            "[overlay] try repo={repo} stanzas=3 builds=5 verdicts keep=2 review=0 drop=1 gaps 4→3 closed=0 surface=engine"
        )]
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("[wrappers] inferred")),
        "the ToLower body infers no wrapper: {lines:#?}"
    );
    std::fs::remove_dir_all(root).ok();
}

/// Every file under `dir` with its length and mtime, sorted.
fn listing(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("read dir") {
            let path = e.expect("entry").path();
            let meta = std::fs::metadata(&path).expect("metadata");
            if meta.is_dir() {
                stack.push(path);
            } else {
                out.push((path, meta.len(), meta.modified().expect("mtime")));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn try_writes_no_layout_and_no_overlay() {
    let root = tmp("nowrite");
    let repo = nightly_repo(&root.join("repo"));
    let overlay = root.join("repo/.glia/overlay.toml");
    std::fs::create_dir_all(overlay.parent().expect("dir")).expect("mkdir");
    std::fs::write(&overlay, FIXTURE_OVERLAY).expect("write");
    let built = generate_one_incremental(&repo).expect("build");
    let layout = default_layout_dir(Path::new(&repo));
    persist_result(&built, &layout, "test").expect("persist");
    let before = listing(&root.join("repo/.glia"));
    assert!(before.len() > 3, "a layout and its sidecar: {before:#?}");

    let report = try_candidate(
        std::slice::from_ref(&repo),
        ENTRYPOINT_AND_EDGE,
        &TryOptions::default(),
    )
    .expect("try");
    assert_eq!(report.builds, 4);
    assert_eq!(
        report.base.nodes_by_kind.get("DATA_ENTITY"),
        Some(&1),
        "BASE is the file's overlay: its wrapper applies"
    );
    assert_eq!(stanza(&report, "entrypoints#1").verdict, KEEP);
    assert_eq!(stanza(&report, "edge#1").verdict, DROP);

    assert_eq!(
        std::fs::read_to_string(&overlay).expect("read"),
        FIXTURE_OVERLAY,
        "try never writes the overlay"
    );
    assert_eq!(
        listing(&root.join("repo/.glia")),
        before,
        "no layout file, and not the unchanged sidecar, was written"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn deterministic() {
    let root = tmp("det");
    let repo = nightly_repo(&root.join("repo"));
    let run = || {
        let r = try_candidate(
            std::slice::from_ref(&repo),
            &three_stanzas(),
            &TryOptions::default(),
        )
        .expect("try");
        serde_json::to_string(&r).expect("json")
    };
    let first = run();
    assert_eq!(first, run());
    assert!(first.contains("\"verdict\":\"keep\""), "{first}");
    std::fs::remove_dir_all(root).ok();
}

// ---------------------------------------------------------------------------
// Propose and accept (CE.3d)
// ---------------------------------------------------------------------------

/// Probe r1's web side: a hand-rolled `request()` whose `fetch(path)` is an
/// `<unresolved>` sink (line 2).
const R1_CLIENT_TS: &str = "export function request(method: string, path: string) {\n  return fetch(path, { method });\n}\n\nexport async function loadUsers() {\n  return request('GET', '/users');\n}\n";
/// Probe r1's api side: two flask routes nothing in the build pairs to.
const R1_API_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef list_users():\n    return []\n\n\n@app.route(\"/orders\", methods=[\"GET\"])\ndef list_orders():\n    return []\n";

fn write_file(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a parent dir")).expect("mkdir");
    std::fs::write(path, body).expect("write");
}

/// Probe r1 as two repos under `root`: `[web, api]`.
fn r1_pair(root: &Path) -> Vec<String> {
    write_file(root, "web/src/client.ts", R1_CLIENT_TS);
    write_file(root, "api/app.py", R1_API_PY);
    ["web", "api"]
        .iter()
        .map(|d| root.join(d).to_str().expect("utf-8").to_string())
        .collect()
}

fn proposed<'p>(p: &'p Proposal, category: &str) -> Vec<&'p GapRow> {
    p.rows
        .iter()
        .filter(|r| r.gap.category == category)
        .map(|r| &r.gap)
        .collect()
}

#[test]
fn propose_lists_rows_with_snippets() {
    let root = tmp("propose");
    let repos = r1_pair(&root);
    let full = propose(&repos, &ProposeOptions::default()).expect("propose");

    assert!(!full.rows.is_empty());
    for r in &full.rows {
        assert!(
            r.gap.id.starts_with("gap:") && r.gap.id.len() == 20,
            "{r:#?}"
        );
        assert_ne!(r.gap.category, WRAPPED_SINK, "wrapped_sink suggests none");
    }
    assert!(!full.counts.contains_key(WRAPPED_SINK), "{:?}", full.counts);

    let sink: Vec<_> = full
        .rows
        .iter()
        .filter(|r| r.gap.category == UNRESOLVED_ENDPOINT)
        .collect();
    assert_eq!(sink.len(), 1, "{:#?}", full.rows);
    assert_eq!(
        (sink[0].gap.file.as_deref(), sink[0].gap.line),
        (Some("src/client.ts"), Some(2))
    );
    let snippet = sink[0]
        .snippet
        .as_ref()
        .expect("one root holds src/client.ts");
    assert_eq!(snippet.file, "src/client.ts");
    assert_eq!(snippet.start_line, 1, "2 - 3, clamped at the file start");
    let want: Vec<&str> = R1_CLIENT_TS.lines().take(5).collect();
    assert_eq!(snippet.lines, want, "lines 1-5");
    assert_eq!(snippet.lines[1], "  return fetch(path, { method });");

    // Every located row in one root carries its snippet.
    for r in &full.rows {
        if r.gap.file.is_some() && r.gap.line.is_some() {
            assert!(r.snippet.is_some(), "{r:#?}");
        }
    }
    let with_snippet = full.rows.iter().filter(|r| r.snippet.is_some()).count();
    assert_eq!(full.snippets, with_snippet);
    assert_eq!(full.ambiguous_root, 0);
    assert!(full.guide.contains("docs/overlay.md"), "{}", full.guide);
    assert_eq!(
        full.counts.get(UNPAIRED_ROUTE),
        Some(&2),
        "{:?}",
        full.counts
    );

    // top_k 1 cuts rows per category; the counts stay the totals.
    let mut one = ProposeOptions::default();
    one.top_k = 1;
    let cut = propose(&repos, &one).expect("propose");
    assert_eq!(cut.counts, full.counts);
    assert_eq!(proposed(&cut, UNPAIRED_ROUTE).len(), 1);
    for c in full.counts.keys() {
        assert!(proposed(&cut, c).len() <= 1, "{c}");
    }
    assert!(cut.rows.len() < full.rows.len());

    // A category list keeps only those categories; an unknown one is an error.
    let mut only = ProposeOptions::default();
    only.categories = vec![UNRESOLVED_ENDPOINT.to_string()];
    let sinks = propose(&repos, &only).expect("propose");
    assert_eq!(sinks.rows.len(), 1);
    assert_eq!(
        sinks.counts.keys().copied().collect::<Vec<_>>(),
        [UNRESOLVED_ENDPOINT]
    );
    let mut bad = ProposeOptions::default();
    bad.categories = vec!["no_such_category".to_string()];
    let err = propose(&repos, &bad).expect_err("unknown category");
    assert!(
        err.contains("no_such_category") && err.contains(UNRESOLVED_ENDPOINT),
        "{err}"
    );

    // The same relative file in both roots: no guess, the snippet is omitted.
    write_file(&root, "api/src/client.ts", "export const VERSION = 1;\n");
    let amb = propose(&repos, &ProposeOptions::default()).expect("propose");
    let in_both: Vec<_> = amb
        .rows
        .iter()
        .filter(|r| r.gap.file.as_deref() == Some("src/client.ts"))
        .collect();
    assert!(
        in_both
            .iter()
            .any(|r| r.gap.category == UNRESOLVED_ENDPOINT),
        "{:#?}",
        amb.rows
    );
    assert!(in_both.iter().all(|r| r.snippet.is_none()), "{in_both:#?}");
    assert_eq!(amb.ambiguous_root, in_both.len());
    for r in amb
        .rows
        .iter()
        .filter(|r| r.gap.file.as_deref() == Some("app.py"))
    {
        assert!(r.snippet.is_some(), "{r:#?}");
    }
    std::fs::remove_dir_all(root).ok();
}

/// A linked gap id for the wrapper stanza (any well-formed id).
const WRAPPER_GAP: &str = "gap:00000000000000a1";

/// The user's own overlay: comments, a pinned constant.
const TEAM_OVERLAY: &str = "\
# team overlay: reviewed by hand
version = 1

# pinned by hand
[constants]
REGION = \"/eu\"  # the EU gateway
";

/// The fixture's wrapper with its `# gap:` link, a pattern and an edge:
/// `wrapper#1`, `entrypoints#1`, `edge#1`.
fn linked_three() -> String {
    format!(
        "# gap: {WRAPPER_GAP}\n{}\n{ENTRYPOINT_AND_EDGE}",
        wrapper_stanza()
    )
}

fn wrapper_only() -> AcceptOptions {
    let mut o = AcceptOptions::default();
    o.only = vec!["wrapper#1".to_string()];
    o
}

fn overlay_of(repo: &str) -> PathBuf {
    Path::new(repo).join(".glia/overlay.toml")
}

#[test]
fn accept_writes_the_chosen_stanzas() {
    let root = tmp("accept");
    let repo = nightly_repo(&root.join("repo"));
    let file = overlay_of(&repo);
    std::fs::create_dir_all(file.parent().expect("dir")).expect("mkdir");
    std::fs::write(&file, TEAM_OVERLAY).expect("write");

    let summary = accept(&repo, Some(&linked_three()), &wrapper_only()).expect("accept");
    assert_eq!(summary.added, BTreeMap::from([("wrapper", 1)]));
    assert_eq!((summary.removed, summary.duplicates), (0, 0));
    assert!(!summary.dry_run && summary.written);
    assert_eq!(summary.file, file.display().to_string());

    let text = std::fs::read_to_string(&file).expect("read");
    assert!(
        text.starts_with(TEAM_OVERLAY),
        "the user's text, comments included, is kept: {text}"
    );
    assert!(
        text.contains(&format!("# gap: {WRAPPER_GAP}\n[[wrapper]]\n")),
        "{text}"
    );
    assert!(text.contains("call = \"NewCollection\""), "{text}");
    assert!(
        !text.contains("[[edge]]") && !text.contains("[entrypoints]"),
        "only wrapper#1 is written: {text}"
    );
    let loaded = glia_config::parse_str(&text);
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    assert_eq!(
        (count(&loaded, "wrapper"), count(&loaded, "constants")),
        (1, 1)
    );
    assert!(
        summary.diff.contains("\n+[[wrapper]]\n"),
        "{}",
        summary.diff
    );
    assert!(
        !summary
            .diff
            .lines()
            .any(|l| l.starts_with('-') && !l.starts_with("---")),
        "nothing removed: {}",
        summary.diff
    );

    // The next build applies it.
    let built = generate_one(&repo).expect("build");
    assert_eq!(entity_repos(&built), [repo.as_str()]);
    assert_eq!(entity_accesses(&built).len(), 1);
    std::fs::remove_dir_all(root).ok();
}

const CHILD_ACCEPT_ENV: &str = "GLIA_CE3D_CHILD_ACCEPT";

/// The child half of [`accept_stderr`]: a no-op unless the parent set
/// [`CHILD_ACCEPT_ENV`], in which case it proposes, accepts `wrapper#1` of
/// [`linked_three`] and builds, so its stderr carries the markers.
#[test]
fn child_accept_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ACCEPT_ENV) {
        propose(std::slice::from_ref(&dir), &ProposeOptions::default()).expect("propose");
        accept(&dir, Some(&linked_three()), &wrapper_only()).expect("accept");
        generate_one(&dir).expect("build");
    }
}

/// The stderr lines of one [`child_accept_for_stderr`] over `dir`.
fn accept_stderr(dir: &str) -> Vec<String> {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "--exact",
            "child_accept_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ACCEPT_ENV, dir)
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "child accept failed: {stderr}");
    stderr.lines().map(String::from).collect()
}

#[test]
fn accept_and_propose_markers() {
    let root = tmp("markers");
    let repo = nightly_repo(&root.join("repo"));
    let lines = accept_stderr(&repo);
    let propose_lines: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("[overlay] propose "))
        .collect();
    assert_eq!(propose_lines.len(), 1, "{lines:#?}");
    assert!(
        propose_lines[0].starts_with(&format!("[overlay] propose repo={repo} rows="))
            && propose_lines[0].ends_with(" ambiguous_root=0 surface=engine"),
        "{}",
        propose_lines[0]
    );
    let accept_lines: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("[overlay] accept "))
        .collect();
    assert_eq!(
        accept_lines,
        [&format!(
            "[overlay] accept repo={repo} added=1 (route_prefix=0 wrapper=1 edge=0 constants=0 entrypoints=0) removed=0 duplicates=0 file=.glia/overlay.toml dry_run=false surface=engine"
        )]
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("[overlay] wrappers ") && l.contains(" minted=1 ")),
        "the next build applies the accepted wrapper: {lines:#?}"
    );
    std::fs::remove_dir_all(root).ok();
}

fn rule_report(repo: &str) -> GapsReport {
    let built = generate_one(repo).expect("build");
    let roots: Vec<(u64, PathBuf)> = built
        .repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect();
    gaps_report(&built.merged, &roots, &GapsOptions::default()).expect("gaps")
}

fn row_of<'r>(rep: &'r GapsReport, category: &str, kind: &str) -> &'r GapRow {
    let found: Vec<&GapRow> = rep
        .rows
        .iter()
        .filter(|r| r.category == category && r.kind == kind)
        .collect();
    assert_eq!(found.len(), 1, "one {category} {kind}: {:#?}", rep.rows);
    found[0]
}

fn remove(ids: &[&str]) -> AcceptOptions {
    let mut o = AcceptOptions::default();
    o.remove = ids.iter().map(|s| s.to_string()).collect();
    o
}

/// The head of [`rot_overlay`]: comments, a pattern that binds and one that
/// does not, and the data wrapper.
fn rot_head() -> String {
    format!(
        "# team overlay\nversion = 1\n\n# started by cron\n[entrypoints]\nqnames = [\n    \"nightly::Nightly\",\n    \"gone::*\",\n]\n\n# the collection helper\n{}",
        wrapper_stanza()
    )
}

/// An [[edge]] between two qnames that do not exist.
const ORPHAN_EDGE: &str = "\n# a call that no longer exists\n[[edge]]\nfrom = \"nowhere::Caller\"\nto = \"nowhere::Callee\"\ncategory = \"CALLS\"\n";
/// A note anchored nowhere, then the file's closing comment.
const NOTE_AND_TAIL: &str =
    "\n[[note]]\nanchor = \"nowhere::Thing\"\ntext = \"retries are safe\"\n\n# end of overlay\n";

/// An [[edge]] equal to the CALLS edge the extractor already emits.
fn redundant_edge(from: &str, to: &str) -> String {
    format!(
        "\n# declared before the extractor caught up\n[[edge]]\nfrom = \"{from}\"\nto = \"{to}\"\ncategory = \"CALLS\"\n"
    )
}

/// `(from, to)` qnames of one CALLS edge the build extracts.
fn extracted_call(repo: &str) -> (String, String) {
    let built = generate_one(repo).expect("build");
    let mut calls: Vec<(String, String)> = built
        .merged
        .all_edges()
        .filter(|e| e.category == edge_category::CALLS)
        .map(|e| (qname_of(&built, e.from), qname_of(&built, e.to)))
        .filter(|(f, t)| !f.is_empty() && !t.is_empty())
        .collect();
    calls.sort();
    calls.into_iter().next().expect("jobs.go calls cleanup")
}

#[test]
fn accept_removes_rot_by_gap_id() {
    let root = tmp("rot");
    let repo = nightly_repo(&root.join("repo"));
    std::fs::write(
        root.join("repo/jobs.go"),
        "package repositories\n\nfunc RunJobs() {\n\tcleanup()\n}\n\nfunc cleanup() {}\n",
    )
    .expect("write");
    let (from, to) = extracted_call(&repo);
    let redundant = redundant_edge(&from, &to);
    let base = format!("{}{ORPHAN_EDGE}{redundant}{NOTE_AND_TAIL}", rot_head());
    let file = overlay_of(&repo);
    std::fs::create_dir_all(file.parent().expect("dir")).expect("mkdir");
    std::fs::write(&file, &base).expect("write");
    assert!(glia_config::parse_str(&base).errors.is_empty());

    let before = rule_report(&repo);
    let orphan_edge = row_of(&before, ORPHANED_RULE, "edge").id.clone();
    let orphan_note = row_of(&before, ORPHANED_RULE, "note").id.clone();
    let orphan_pattern = row_of(&before, ORPHANED_RULE, "entrypoint");
    assert_eq!(orphan_pattern.qname, "gone::*");
    let orphan_pattern = orphan_pattern.id.clone();
    let redundant_id = row_of(&before, REDUNDANT_RULE, "edge").id.clone();

    // An unknown id, and an id of another category, are refused; nothing written.
    let err = accept(&repo, None, &remove(&["gap:0000000000000000"])).expect_err("unknown id");
    assert!(err.contains("gap:0000000000000000"), "{err}");
    let dead = before
        .rows
        .iter()
        .find(|r| r.category == DEAD_SYMBOL)
        .expect("the Go sources have a dead symbol");
    let err = accept(&repo, None, &remove(&[&dead.id])).expect_err("not a rule row");
    assert!(err.contains(&dead.id) && err.contains(DEAD_SYMBOL), "{err}");
    assert_eq!(std::fs::read_to_string(&file).expect("read"), base);

    // The orphaned [[edge]] goes, by its identity, and nothing else does.
    let summary = accept(&repo, None, &remove(&[&orphan_edge])).expect("accept");
    assert_eq!((summary.removed, summary.added.len()), (1, 0));
    let after = std::fs::read_to_string(&file).expect("read");
    assert_eq!(
        after,
        format!("{}{redundant}{NOTE_AND_TAIL}", rot_head()),
        "exactly the stanza and its comment are gone"
    );
    let rerun = rule_report(&repo);
    assert!(rerun.rows.iter().all(|r| r.id != orphan_edge), "{rerun:#?}");
    for id in [&orphan_note, &orphan_pattern, &redundant_id] {
        assert!(rerun.rows.iter().any(|r| &r.id == id), "{id} stays");
    }

    // The rest of the rot in one call.
    let summary = accept(
        &repo,
        None,
        &remove(&[&redundant_id, &orphan_note, &orphan_pattern]),
    )
    .expect("accept");
    assert_eq!(summary.removed, 3);
    let text = std::fs::read_to_string(&file).expect("read");
    let loaded = glia_config::parse_str(&text);
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    assert_eq!(
        (
            count(&loaded, "edge"),
            count(&loaded, "note"),
            count(&loaded, "entrypoints"),
            count(&loaded, "wrapper"),
        ),
        (0, 0, 1, 1)
    );
    assert!(
        text.starts_with("# team overlay\nversion = 1\n\n# started by cron\n[entrypoints]\n"),
        "{text}"
    );
    assert!(text.contains("\"nightly::Nightly\"") && !text.contains("gone::*"));
    assert!(text.ends_with("# end of overlay\n"), "{text}");
    let clean = rule_report(&repo);
    assert_eq!(
        (clean.count(ORPHANED_RULE), clean.count(REDUNDANT_RULE)),
        (0, 0),
        "{clean:#?}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn accept_removes_twins_together() {
    let root = tmp("twins");
    let repo = nightly_repo(&root.join("repo"));
    let base = format!("version = 1\n{ORPHAN_EDGE}{ORPHAN_EDGE}");
    let file = overlay_of(&repo);
    std::fs::create_dir_all(file.parent().expect("dir")).expect("mkdir");
    std::fs::write(&file, &base).expect("write");
    let before = rule_report(&repo);
    let twins: Vec<&str> = before
        .rows
        .iter()
        .filter(|r| r.category == ORPHANED_RULE)
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(twins.len(), 2, "{before:#?}");
    assert_ne!(twins[0], twins[1]);

    // One id names the identity: both identical stanzas go, both counted.
    let summary = accept(&repo, None, &remove(&[twins[0]])).expect("accept");
    assert_eq!(summary.removed, 2);
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "version = 1\n"
    );
    assert_eq!(rule_report(&repo).count(ORPHANED_RULE), 0);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn accept_refuses_a_bad_candidate() {
    let root = tmp("refuse");
    let repo = nightly_repo(&root.join("repo"));
    let file = overlay_of(&repo);
    std::fs::create_dir_all(file.parent().expect("dir")).expect("mkdir");
    std::fs::write(&file, TEAM_OVERLAY).expect("write");

    // The loader drops a route_prefix without a leading `/`: refused, quoted.
    let bad = "[[route_prefix]]\nscope = \"orders\"\nprefix = \"orders\"\n";
    let err = accept(&repo, Some(bad), &AcceptOptions::default()).expect_err("refused");
    assert!(
        err.contains("prefix \"orders\" must start with '/'"),
        "the loader's own words: {err}"
    );
    assert_eq!(std::fs::read(&file).expect("read"), TEAM_OVERLAY.as_bytes());

    // An unknown `only` ref lists the candidate's refs.
    let mut unknown = AcceptOptions::default();
    unknown.only = vec!["wrapper#9".to_string()];
    let err = accept(&repo, Some(&linked_three()), &unknown).expect_err("unknown ref");
    assert!(
        err.contains("wrapper#9")
            && err.contains("wrapper#1")
            && err.contains("entrypoints#1")
            && err.contains("edge#1"),
        "{err}"
    );

    // Nothing to accept.
    let err = accept(&repo, None, &AcceptOptions::default()).expect_err("no input");
    assert!(err.contains("nothing to accept"), "{err}");

    // A dry run returns the diff and writes nothing.
    let mut dry = wrapper_only();
    dry.dry_run = true;
    let summary = accept(&repo, Some(&linked_three()), &dry).expect("dry run");
    assert!(summary.dry_run && !summary.written);
    assert_eq!(summary.added, BTreeMap::from([("wrapper", 1)]));
    assert!(
        summary
            .diff
            .starts_with("--- a/.glia/overlay.toml\n+++ b/.glia/overlay.toml\n@@ "),
        "{}",
        summary.diff
    );
    assert!(
        summary.diff.contains("\n+[[wrapper]]\n"),
        "{}",
        summary.diff
    );
    assert_eq!(std::fs::read(&file).expect("read"), TEAM_OVERLAY.as_bytes());
    assert_eq!(
        std::fs::read_dir(file.parent().expect("dir"))
            .expect("read dir")
            .count(),
        1,
        "no temp file left behind"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn accept_creates_the_file() {
    let root = tmp("create");
    let repo = nightly_repo(&root.join("repo"));
    assert!(!Path::new(&repo).join(".glia").exists());
    let cand = format!("# gap: {WRAPPER_GAP}\n{}", wrapper_stanza());
    let summary = accept(&repo, Some(&cand), &AcceptOptions::default()).expect("accept");
    assert!(summary.written);
    assert_eq!(summary.added, BTreeMap::from([("wrapper", 1)]));
    assert!(
        summary
            .diff
            .starts_with("--- /dev/null\n+++ b/.glia/overlay.toml\n@@ -0,0 +1,"),
        "{}",
        summary.diff
    );
    let file = overlay_of(&repo);
    let text = std::fs::read_to_string(&file).expect("the file was created");
    assert!(text.starts_with("version = 1"), "{text}");
    let loaded = glia_config::parse_str(&text);
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    assert_eq!(count(&loaded, "wrapper"), 1);
    let names: Vec<String> = std::fs::read_dir(file.parent().expect("dir"))
        .expect("read dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["overlay.toml"], "no temp file left behind");
    std::fs::remove_dir_all(root).ok();
}
