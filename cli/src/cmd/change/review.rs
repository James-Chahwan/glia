//! `glia review` (CC.6b) — the PR report for a change, as a CI gate: the
//! human surface over the engine's `review::review_vs_rev` (CC.6a), the git
//! rev `--base` (default `HEAD`) against the working tree, in one call — the
//! changed nodes, their ranked impact, the tests to run, every added /
//! removed edge with its `why` tier, and the working tree's declared rules
//! (`.glia/overlay.toml` `[[constraint]]`) checked on both sides.
//!
//! Default stdout is the engine's `review::render_markdown` (the same text
//! pyo3 `review_vs_rev(format="markdown")` returns, for a PR comment), its
//! tables cut at `--markdown-rows` with `_(<shown> of <total>)_`. `--json`
//! prints the engine's `Review` (`{base, counts, changed, impact, tests,
//! edges, new_violations, resolved_violations, model_breaks, check_errors,
//! blocking}`, plus `unanchored` when a rule checks nothing and `intent`
//! with a title).
//! `--depth` bounds the impact radius; `--max-impact` / `--max-tests` cut
//! the engine's lists (the counts stay uncut). Every `file:line` is 1-based
//! (LD.1).
//!
//! Exit 1 when the change adds a BLOCKING violation or a BLOCKING model
//! break (`blocking`), in both modes: a new violation at or above its rule's
//! `min_tier` (default `derived`), never a heuristic one (0.5.2 Q2), or a
//! breaking inferred client <-> server model row (DT.1p, the engine's
//! `contract_breaks::model_breaks`) whose pairing tier is at or above
//! `--models-min-tier` (`fact`, the default, or `derived`; every pairing glia
//! 0.5.2 infers is derived, so by default a model break is advisory). A new
//! advisory violation or model break is listed (markdown: under `### New
//! violations (advisory: below the rule's min_tier)` / `### Model breaks
//! (advisory: an inferred client <-> server pairing below the models min
//! tier)`, and ` (<A> advisory)` in the headline) and does not fail the
//! gate; nor does a violation the base already had (`glia check` fails on
//! any blocking one). A rule that checks nothing in the working tree (a
//! scope no located node sits in; DT.7b / DT.7c) is listed under `### Rules
//! that check nothing` with its fix, and never fails the gate. Rule-invariant and transitive (forbid_path) violations
//! are new or resolved by their subject, not their witness rows. 0
//! otherwise. 2 on `--no-overlay` (both sides are built with the
//! repo's overlay, as for `glia delta`) or a git / build failure, with the
//! engine's message. The rev pair is one repo, so there is no `--with`
//! (clap refuses it, exit 2).
//!
//! Writes: the working tree's parse-cache sidecar
//! (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored), as `glia delta`
//! does, under `GLIA_NO_PERSIST=1` too; never a `.gmap` layout.
//!
//! 0.5.3 (EA.8): `--ranking ppr|ppr-wide|bfs` and `--no-cochange` pick the
//! file ranking that orders the impact rows (`ReviewArgs.blast.ranking`;
//! co-change from the working tree's `.glia/history-snapshot` first by
//! default). The markdown is unchanged: its impact table follows the rows'
//! order. `[rank] surface=review ranking=<r> ..` once per run.
//!
//! 0.5.3 (EB.7): `--title <TEXT>` is the PR's purpose (default: the newest
//! commit subject of `<base>..HEAD`; none when the range is empty). Its ids
//! route at the base through the repo's task / requirement rows (typed id
//! routing) and its words rank past commit messages; the JSON gains `intent`
//! and the markdown `### Intent` (the routed ids, the files ranked
//! LX[IDR,P1]), and the impact rows in id-routed files move first (the rest
//! keep their order; text never moves a row). `[review-intent] base=<rev>
//! title_source=<arg|commit|none> fusion=LX[IDR,P1] ..` after `[review]
//! base=`.
//!
//! Fired-on markers: the engine's
//! `[model-breaks] base=<rev> min_tier=<t> exchanges=<E> .. unjudged=<U>`
//! and
//! `[review] base=<rev> changed=<C> seeds=<S> impact=<I> tests=<T> untested=<U> edges +<a> -<r> (fact=<F> derived=<D> heuristic=<H>) violations new=<N> resolved=<R> blocking=<bool>`
//! once per run (grep `^\[review\] base=`), then this surface's
//! `[review] surface=cli format=<markdown|json> exit=<0|1>`. In markdown, a
//! rule's coverage caveats print once per section (DT.8a's fold, the
//! engine's `review::caveat_fold`); when a caveat block printed, after the
//! report: `[review] caveats rules=<r> lines=<l> folded=<f>` (`r` the blocks,
//! `l` their `⚠` lines, `f` the lines a block per violation would have
//! printed beyond `l`).

use glia_engine::contract_breaks::MODEL_MIN_TIERS;
use glia_engine::review::{
    DEFAULT_MARKDOWN_ROWS, DEFAULT_MAX_IMPACT, DEFAULT_MAX_TESTS, MarkdownOptions, ReviewArgs,
    caveat_fold, render_markdown, review_vs_rev,
};

use glia_engine::rank::Ranking;

use crate::common::{RankingArgs, build_options};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo (a git work tree).
    repo: String,
    /// The git rev to compare the working tree against: a branch, tag, sha
    /// or `HEAD~N`.
    #[arg(long, value_name = "REV", default_value = "HEAD")]
    base: String,
    /// Emit the review as JSON (`{base, counts, changed, impact, tests,
    /// edges, new_violations, resolved_violations, model_breaks,
    /// check_errors, blocking}`) instead of the markdown report.
    #[arg(long)]
    json: bool,
    /// Rows per markdown table; a cut table ends with `_(<shown> of
    /// <total>)_`.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_MARKDOWN_ROWS)]
    markdown_rows: usize,
    /// Maximum hops of the impact radius around the changed nodes.
    #[arg(long, value_name = "N", default_value_t = ReviewArgs::default().blast.depth)]
    depth: usize,
    /// Test rows the review keeps (`counts.tests` stays the uncut total).
    #[arg(long, value_name = "N", default_value_t = DEFAULT_MAX_TESTS)]
    max_tests: usize,
    /// Impact rows the review keeps (`counts.impact` stays the uncut total).
    #[arg(long, value_name = "N", default_value_t = DEFAULT_MAX_IMPACT)]
    max_impact: usize,
    /// The weakest pairing tier at which an inferred client <-> server model
    /// break fails the gate: fact (the default; every pairing glia 0.5.2
    /// infers is derived, so its rows are advisory) or derived; a heuristic
    /// pairing never fails it.
    #[arg(
        long,
        value_name = "TIER",
        default_value = MODEL_MIN_TIERS[0],
        value_parser = clap::builder::PossibleValuesParser::new(MODEL_MIN_TIERS),
    )]
    models_min_tier: String,
    /// The PR title: its ids route through the repo's task / requirement rows
    /// and its words rank past commits; default: the newest commit subject of
    /// <base>..HEAD
    #[arg(long, value_name = "TEXT")]
    title: Option<String>,
    #[command(flatten)]
    ranking: RankingArgs,
}

/// The engine's arguments from the flags; `Err` names a tier outside
/// [`MODEL_MIN_TIERS`] in the engine's words (clap has already refused one,
/// so this only guards the `&'static str` lookup).
fn engine_args(
    depth: usize,
    max_tests: usize,
    max_impact: usize,
    models_min_tier: &str,
    ranking: Ranking,
    title: Option<String>,
) -> Result<ReviewArgs, String> {
    let Some(tier) = MODEL_MIN_TIERS.iter().copied().find(|t| *t == models_min_tier) else {
        return Err(format!(
            "unknown models min tier `{models_min_tier}`: expected one of {}",
            MODEL_MIN_TIERS.join(", ")
        ));
    };
    let mut args = ReviewArgs::default();
    args.blast.depth = depth;
    args.blast.ranking = ranking;
    args.max_tests = max_tests;
    args.max_impact = max_impact;
    args.models_min_tier = tier;
    args.title = title;
    Ok(args)
}

pub(crate) fn run(args: Args) -> i32 {
    if !build_options().overlay {
        eprintln!(
            "error: --no-overlay does not apply to review: both sides are built with the repo's \
             overlay, and its rules are the working tree's"
        );
        return 2;
    }
    let engine = match engine_args(
        args.depth,
        args.max_tests,
        args.max_impact,
        &args.models_min_tier,
        args.ranking.ranking(),
        args.title.clone(),
    ) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let review = match review_vs_rev(&args.repo, &args.base, &engine) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let exit = i32::from(review.blocking);
    let format = if args.json { "json" } else { "markdown" };
    eprintln!("[review] surface=cli format={format} exit={exit}");
    if args.json {
        match serde_json::to_string(&review) {
            Ok(text) => println!("{text}"),
            Err(e) => {
                eprintln!("error: serialising the review: {e}");
                return 2;
            }
        }
    } else {
        let mut opts = MarkdownOptions::default();
        opts.max_rows = args.markdown_rows;
        print!("{}", render_markdown(&review, &opts));
        let fold = caveat_fold(&review);
        if fold.rules > 0 {
            eprintln!(
                "[review] caveats rules={} lines={} folded={}",
                fold.rules, fold.lines, fold.folded
            );
        }
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_args_carry_the_flags() {
        let bfs = Ranking::new(glia_engine::rank::RankBase::Bfs, false);
        let a = engine_args(2, 7, 9, "derived", bfs, Some("T012 export".to_string())).expect("a tier");
        assert_eq!(
            (a.blast.depth, a.max_tests, a.max_impact, a.models_min_tier, a.blast.ranking),
            (2, 7, 9, "derived", bfs)
        );
        assert_eq!(a.title.as_deref(), Some("T012 export"));
        let e = ReviewArgs::default();
        let d = engine_args(e.blast.depth, DEFAULT_MAX_TESTS, DEFAULT_MAX_IMPACT, "fact", Ranking::default(), None)
            .expect("the default tier");
        assert_eq!(
            (d.blast.depth, d.max_tests, d.max_impact, d.models_min_tier, d.blast.ranking, d.title),
            (e.blast.depth, e.max_tests, e.max_impact, e.models_min_tier, e.blast.ranking, e.title)
        );
        assert_eq!(e.models_min_tier, "fact", "the ruling's default");
        let err = engine_args(4, 50, 50, "heuristic", Ranking::default(), None).expect_err("no such gate");
        assert_eq!(err, "unknown models min tier `heuristic`: expected one of fact, derived");
    }
}
