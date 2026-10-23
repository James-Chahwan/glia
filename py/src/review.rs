//! **review** (CC.6b): the review report against a git rev, over the
//! engine's `review::{review_vs_rev, render_markdown}` (CC.6a / CC.6b) — the
//! pyo3 twin of `glia review`.
//!
//! The module function `review_vs_rev(repo_path, base="HEAD", format="dict",
//! markdown_rows=20, depth=4, max_tests=50, max_impact=50,
//! models_min_tier="fact", ranking=None, cochange=True, title=None)` builds the repo's
//! working tree and the rev itself (LE.1b's graph delta), so it takes a path.
//! `format="dict"` returns the engine's `Review` as a native dict (LD.2):
//! `{base, counts, changed, impact, tests, edges, new_violations,
//! resolved_violations, model_breaks, check_errors, blocking}` (plus
//! `unanchored` when a rule checks nothing, and `intent` with a title or a
//! commit in `<base>..HEAD`, EB.7); `format="markdown"` returns the
//! markdown PR report `glia review` prints, as a `str`. Any other `format`, a
//! `models_min_tier` outside the engine's `contract_breaks::MODEL_MIN_TIERS`,
//! an unknown `ranking`, or an engine `Err` (not a git work tree, an unknown
//! rev, a failed build), raises `ValueError`; the format, the tier and the
//! ranking are checked before anything is built.
//!
//! Transport only: the review, the renderer, the location of every row and
//! the `[review] base=..` marker live in the engine; this module adds one
//! `[review] surface=py format=<dict|markdown>` line per call. The helpers the
//! pyo3 entry point delegates to are pyo3-free, so `cargo test -p glia-py`
//! covers them (see the crate doc).

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::contract_breaks::MODEL_MIN_TIERS;
use glia_engine::review::{
    DEFAULT_MARKDOWN_ROWS, DEFAULT_MAX_IMPACT, DEFAULT_MAX_TESTS, MarkdownOptions, Review,
    ReviewArgs, render_markdown, review_vs_rev,
};

use crate::blast::ranking_selected;
use crate::convert::to_py;
use crate::registry::ModuleFns;

/// The `format` values, the default first.
const FORMATS: [&str; 2] = ["dict", "markdown"];

/// What [`rev_answer`] hands back: the review, or its markdown.
#[derive(Debug)]
enum Answer {
    Dict(Box<Review>),
    Markdown(String),
}

/// The engine's arguments from the keyword arguments; `Err` names a
/// `models_min_tier` outside [`MODEL_MIN_TIERS`] in the engine's own words,
/// or an unknown `ranking` ([`ranking_selected`], EA.7 / EC.8: `None`
/// applies the calibration).
fn engine_args(
    depth: usize,
    max_tests: usize,
    max_impact: usize,
    models_min_tier: &str,
    ranking: Option<&str>,
    cochange: bool,
    title: Option<&str>,
) -> Result<ReviewArgs, String> {
    let (ranking, select) = ranking_selected(ranking, cochange)?;
    let Some(tier) = MODEL_MIN_TIERS.iter().copied().find(|t| *t == models_min_tier) else {
        return Err(format!(
            "unknown models min tier `{models_min_tier}`: expected one of {}",
            MODEL_MIN_TIERS.join(", ")
        ));
    };
    let mut args = ReviewArgs::default();
    args.blast.depth = depth;
    args.max_tests = max_tests;
    args.max_impact = max_impact;
    args.models_min_tier = tier;
    args.blast.ranking = ranking;
    args.blast.select = select;
    args.title = title.map(str::to_string);
    Ok(args)
}

/// `format` as one of [`FORMATS`]; `Err` names it and the valid ones.
fn check_format(format: &str) -> Result<&'static str, String> {
    FORMATS
        .iter()
        .copied()
        .find(|f| *f == format)
        .ok_or_else(|| {
            format!(
                "unknown format `{format}`: expected one of {}",
                FORMATS.join(", ")
            )
        })
}

/// The keyword arguments of one call (the pyo3 signature's, minus the path
/// and the rev).
struct Kw<'a> {
    format: &'a str,
    markdown_rows: usize,
    depth: usize,
    max_tests: usize,
    max_impact: usize,
    models_min_tier: &'a str,
    ranking: Option<&'a str>,
    cochange: bool,
    title: Option<&'a str>,
}

impl Default for Kw<'_> {
    /// The pyo3 signature's defaults.
    fn default() -> Self {
        Kw {
            format: FORMATS[0],
            markdown_rows: DEFAULT_MARKDOWN_ROWS,
            depth: ReviewArgs::default().blast.depth,
            max_tests: DEFAULT_MAX_TESTS,
            max_impact: DEFAULT_MAX_IMPACT,
            models_min_tier: MODEL_MIN_TIERS[0],
            ranking: None,
            cochange: true,
            title: None,
        }
    }
}

/// The body of [`review_vs_rev_py`], minus pyo3. The format and the tier are
/// checked before anything is built.
fn rev_answer(repo_path: &str, base: &str, kw: &Kw<'_>) -> Result<Answer, String> {
    let format = check_format(kw.format)?;
    let args = engine_args(kw.depth, kw.max_tests, kw.max_impact, kw.models_min_tier, kw.ranking, kw.cochange, kw.title)?;
    let review = review_vs_rev(repo_path, base, &args)?;
    eprintln!("[review] surface=py format={format}");
    if format == FORMATS[0] {
        return Ok(Answer::Dict(Box::new(review)));
    }
    let mut opts = MarkdownOptions::default();
    opts.max_rows = kw.markdown_rows;
    Ok(Answer::Markdown(render_markdown(&review, &opts)))
}

/// **review_vs_rev** (CC.6b): the review of the working tree's change against
/// git rev `base` (a branch, tag, sha or `"HEAD~N"`; default `"HEAD"`) of the
/// repo at `repo_path`, in one call: the changed nodes, their ranked impact
/// (`depth` hops, `max_impact` rows), the tests to run (`max_tests` rows),
/// every added / removed / reconfidenced edge with its tier (`fact`,
/// `derived`, `heuristic`), and the working tree's declared rules
/// (`.glia/overlay.toml` `[[constraint]]`) checked on both sides.
///
/// `format="dict"` (default) returns a dict `{base, counts, changed, impact,
/// tests, edges, new_violations, resolved_violations, model_breaks,
/// check_errors, blocking}` (plus `unanchored`, the rules that check nothing,
/// `{rule_id, section, decl, repo, status, reason, fix, blocking}`, when
/// any): `counts` uncut by the row caps; `changed` rows `{id, qname,
/// kind, file, line, change, seed}`; `impact` is `blast_radius`'s dict;
/// `tests` is `tests_for`'s; `edges` rows `{change, from_qname, to_qname,
/// category, tier, note, site_file, site_line, emitter}`; each violation
/// `check`'s `{rule_id, rule_kind, decl, severity, tier, blocking, count,
/// evidence}` (plus `subject`, `file`, `line`, `caveats` on a rule or
/// forbid_path violation), a per-edge one reduced to its new (or resolved)
/// rows with its `tier` and `blocking` read from them; a rule or forbid_path
/// violation is new or resolved by its `subject`; `check_errors` `(rule id,
/// message)` pairs; `model_breaks` (DT.1p) the breaking inferred client <->
/// server model rows, as `contract_breaks_vs_rev`'s `models` rows;
/// `blocking` is True when a new BLOCKING violation exists: one at or above
/// its rule's `min_tier` (default `derived`), never a heuristic one, or a
/// model row blocks: its pairing tier is at or above `models_min_tier`
/// (`"fact"`, the default, or `"derived"`; every pairing glia 0.5.2 infers is
/// derived, so by default a model break is advisory). A CI gate fails on it;
/// an advisory new violation or model break is listed but does not block,
/// and a violation the base already had never blocks.
/// `format="markdown"`
/// returns the markdown PR report `glia review` prints (`str`), each table
/// cut at `markdown_rows`. Lines are 1-based. Builds both sides itself and
/// saves the parse-cache sidecar (`<repo>/.glia/graph/parse_cache.bin`,
/// self-gitignored) as `generate(incremental=True)` does, never a `.gmap`
/// layout. Raises ValueError on another `format`, a `models_min_tier` other
/// than `"fact"` / `"derived"` (`unknown models min tier `x`: expected one of
/// fact, derived`, before any build), or a git or build failure.
///
/// `ranking` (0.5.3; `ppr`, `ppr-wide` or `bfs`) and `cochange` (default
/// `True`: co-change from the working tree's history snapshot first) order
/// the impact rows and fill `impact.files`, as in `blast_radius`; an unknown
/// `ranking` raises ValueError before any build. `ranking=None` (the
/// default, EC.8) applies the working tree's calibrated pick, else `ppr`,
/// and `impact` gains `selection`.
///
/// `title` (0.5.3, EB.7; default None: the newest commit subject of
/// `base..HEAD`, and no intent when that range is empty) is the PR's
/// purpose: its ids route at the base through the repo's task / requirement
/// rows and its words rank past commit messages. The dict gains `intent`
/// `{title, title_source, fusion, ids, files, text}` (`files` rows `{rank,
/// file, idr, text, ppr, ids, in_impact}`) and then `selection_intent`
/// `{set, shape, pick, source, reason, test_commits}` (EC.11): `fusion` is
/// set `SELt`'s pick, the repo's calibrated text fusion for the shape of the
/// changed files (`glia history calibrate`), else `"LX[IDR,P1]"` with the
/// reason; the markdown gains `### Intent` with its `_ranked by .._` line,
/// and the impact rows in id-routed files come first (text never reorders
/// them).
#[pyfunction]
#[pyo3(name = "review_vs_rev", signature = (repo_path, base="HEAD", format="dict", markdown_rows=20, depth=4, max_tests=50, max_impact=50, models_min_tier="fact", ranking=None, cochange=true, title=None))]
#[allow(clippy::too_many_arguments)]
fn review_vs_rev_py(
    py: Python<'_>,
    repo_path: &str,
    base: &str,
    format: &str,
    markdown_rows: usize,
    depth: usize,
    max_tests: usize,
    max_impact: usize,
    models_min_tier: &str,
    ranking: Option<&str>,
    cochange: bool,
    title: Option<&str>,
) -> PyResult<Py<PyAny>> {
    let kw = Kw {
        format,
        markdown_rows,
        depth,
        max_tests,
        max_impact,
        models_min_tier,
        ranking,
        cochange,
        title,
    };
    let answer = rev_answer(repo_path, base, &kw).map_err(PyValueError::new_err)?;
    match answer {
        Answer::Dict(review) => to_py(py, serde_json::to_string(&review)),
        Answer::Markdown(text) => Ok(text.into_pyobject(py)?.into_any().unbind()),
    }
}

fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(review_vs_rev_py, m)?)?;
    Ok(())
}

inventory::submit! { ModuleFns { name: "review", add: register } }

/// The pyo3 signature's defaults are literals (the macro takes no paths):
/// these pin them to the engine's.
const _: () = {
    assert!(DEFAULT_MARKDOWN_ROWS == 20);
    assert!(DEFAULT_MAX_TESTS == 50);
    assert!(DEFAULT_MAX_IMPACT == 50);
};

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;

    /// A scratch dir under the system temp dir, removed on drop (`py` has no
    /// `tempfile` dev-dependency).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// CC.6a's fixture: two manifest projects, a rule forbidding web ->
    /// services/api, and `web/app.py` importing and calling the api.
    const FILES: [(&str, &str); 5] = [
        ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
        ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
        ("services/api/internal.py", "def charge(o):\n    return o\n"),
        ("web/app.py", "def pay(o):\n    return o\n"),
        (
            ".glia/overlay.toml",
            "version = 1\n\n[[constraint]]\nid = \"web-no-api-internals\"\nkind = \"forbid_edge\"\nfrom = \"web\"\nto = \"services/api\"\ncategories = [\"IMPORTS\", \"CALLS\"]\n",
        ),
    ];
    const WEB_APP: &str =
        "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n";

    fn git(top: &Path, gitconfig: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=glia",
                "-c",
                "user.email=glia@example.invalid",
            ])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .arg("-C")
            .arg(top)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap_or_else(|e| panic!("CC.6b review_vs_rev test needs a `git` binary: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The fixture committed clean, then `web/app.py` violating the rule in
    /// the working tree.
    fn violating(name: &str) -> (Scratch, PathBuf) {
        let root = std::env::temp_dir().join(format!("glia-cc6b-py-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        for (rel, text) in FILES {
            let p = top.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("scratch dir");
            std::fs::write(&p, text).expect("write fixture");
        }
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        git(&top, &gitconfig, &["init", "-q"]);
        git(&top, &gitconfig, &["add", "-A"]);
        git(&top, &gitconfig, &["commit", "-q", "-m", "clean"]);
        std::fs::write(top.join("web/app.py"), WEB_APP).expect("edit");
        (Scratch(root), top)
    }

    /// CC.6b: the helper behind `review_vs_rev` returns the review for
    /// `dict`, the engine's markdown for `markdown`, and refuses any other
    /// format before building; the keywords reach the engine.
    #[test]
    fn rev_helper_returns_dict_or_markdown() {
        let (_scratch, top) = violating("formats");
        let repo = top.to_str().expect("utf-8");
        let Ok(Answer::Dict(r)) = rev_answer(repo, "HEAD", &Kw::default()) else {
            panic!("format=dict is the review");
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&r).expect("serialises")).expect("JSON");
        let keys: Vec<&str> = v
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        let mut want = vec![
            "base",
            "counts",
            "changed",
            "impact",
            "tests",
            "edges",
            "new_violations",
            "resolved_violations",
            "model_breaks",
            "check_errors",
            "blocking",
        ];
        want.sort_unstable();
        assert_eq!(keys, want, "serde_json's map sorts keys");
        assert_eq!(
            (v["base"].as_str(), v["blocking"].as_bool()),
            (Some("HEAD"), Some(true))
        );

        let markdown = Kw {
            format: "markdown",
            ..Kw::default()
        };
        let Ok(Answer::Markdown(text)) = rev_answer(repo, "HEAD", &markdown) else {
            panic!("format=markdown is the report");
        };
        assert!(
            text.starts_with("## glia review vs `HEAD`\n**1 new violation(s)**"),
            "{text}"
        );
        assert_eq!(text, render_markdown(&r, &MarkdownOptions::default()));

        let one_row = Kw {
            format: "markdown",
            markdown_rows: 1,
            ..Kw::default()
        };
        let Ok(Answer::Markdown(cut)) = rev_answer(repo, "HEAD", &one_row) else {
            panic!("format=markdown is the report");
        };
        assert!(
            cut.contains("_(1 of 2)_"),
            "markdown_rows reaches the renderer:\n{cut}"
        );

        let no_rows = Kw {
            max_tests: 0,
            max_impact: 0,
            ..Kw::default()
        };
        let Ok(Answer::Dict(capped)) = rev_answer(repo, "HEAD", &no_rows) else {
            panic!("format=dict is the review");
        };
        assert!(capped.tests.tests.is_empty() && capped.impact.results.is_empty());
        assert_eq!(
            (capped.counts.tests, capped.counts.impact),
            (r.counts.tests, r.counts.impact)
        );

        let xml = Kw {
            format: "xml",
            ..Kw::default()
        };
        let err = rev_answer(repo, "HEAD", &xml).expect_err("xml is no format");
        assert_eq!(err, "unknown format `xml`: expected one of dict, markdown");
        let err = rev_answer(repo, "no-such-rev", &Kw::default()).expect_err("unknown rev");
        assert!(err.contains("no-such-rev"), "{err}");
        let x = Kw {
            models_min_tier: "x",
            ..Kw::default()
        };
        let err = rev_answer("/nonexistent/dt1p", "HEAD", &x).expect_err("bad tier");
        assert_eq!(
            err, "unknown models min tier `x`: expected one of fact, derived",
            "refused before any build: the path is never read"
        );
        let bad = Kw {
            ranking: Some("x"),
            ..Kw::default()
        };
        let err = rev_answer("/nonexistent/ea7", "HEAD", &bad).expect_err("bad ranking");
        assert_eq!(
            err, "unknown ranking `x`: expected one of ppr, ppr-wide, bfs",
            "refused before any build: the path is never read"
        );
    }

    #[test]
    fn engine_args_carry_the_keywords() {
        let a = engine_args(2, 7, 9, "derived", Some("ppr-wide"), false, Some("T012 export")).expect("a tier");
        assert_eq!(
            (a.blast.depth, a.max_tests, a.max_impact, a.models_min_tier, a.blast.ranking.harness_name(), a.blast.select),
            (2, 7, 9, "derived", "PT*", false)
        );
        let c = engine_args(4, 50, 50, "fact", None, true, None).expect("calibrated");
        assert_eq!((c.blast.ranking.harness_name(), c.blast.select), ("H10", true), "EC.8: None selects");
        assert_eq!(a.title.as_deref(), Some("T012 export"), "EB.7: title reaches the engine");
        let d = ReviewArgs::default();
        assert_eq!(d.blast.depth, 4, "the signature's depth default");
        assert_eq!(d.models_min_tier, "fact", "the signature's models_min_tier default");
        assert_eq!(d.blast.ranking.harness_name(), "H10", "the signature's ranking / cochange defaults");
        let k = Kw::default();
        assert_eq!(
            (k.format, k.markdown_rows, k.depth, k.max_tests, k.max_impact, k.models_min_tier, k.ranking, k.cochange, k.title),
            ("dict", 20, 4, 50, 50, "fact", None, true, None),
            "Kw::default is the pyo3 signature's"
        );
    }
}
