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
//! edges, new_violations, resolved_violations, check_errors, blocking}`).
//! `--depth` bounds the impact radius; `--max-impact` / `--max-tests` cut
//! the engine's lists (the counts stay uncut). Every `file:line` is 1-based
//! (LD.1).
//!
//! Exit 1 when the change adds a violation (`blocking`), in both modes: a
//! violation the base already had does not fail the gate (`glia check` fails
//! on any). 0 otherwise. 2 on `--no-overlay` (both sides are built with the
//! repo's overlay, as for `glia delta`) or a git / build failure, with the
//! engine's message. The rev pair is one repo, so there is no `--with`
//! (clap refuses it, exit 2).
//!
//! Writes: the working tree's parse-cache sidecar
//! (`<repo>/.glia/graph/parse_cache.bin`, self-gitignored), as `glia delta`
//! does, under `GLIA_NO_PERSIST=1` too; never a `.gmap` layout.
//!
//! Fired-on markers: the engine's
//! `[review] base=<rev> changed=<C> seeds=<S> impact=<I> tests=<T> untested=<U> edges +<a> -<r> (fact=<F> derived=<D> heuristic=<H>) violations new=<N> resolved=<R> blocking=<bool>`
//! once per run (grep `^\[review\] base=`), then this surface's
//! `[review] surface=cli format=<markdown|json> exit=<0|1>`.

use glia_engine::review::{
    DEFAULT_MARKDOWN_ROWS, DEFAULT_MAX_IMPACT, DEFAULT_MAX_TESTS, MarkdownOptions, ReviewArgs,
    render_markdown, review_vs_rev,
};

use crate::common::build_options;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo (a git work tree).
    repo: String,
    /// The git rev to compare the working tree against: a branch, tag, sha
    /// or `HEAD~N`.
    #[arg(long, value_name = "REV", default_value = "HEAD")]
    base: String,
    /// Emit the review as JSON (`{base, counts, changed, impact, tests,
    /// edges, new_violations, resolved_violations, check_errors, blocking}`)
    /// instead of the markdown report.
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
}

/// The engine's arguments from the flags.
fn engine_args(depth: usize, max_tests: usize, max_impact: usize) -> ReviewArgs {
    let mut args = ReviewArgs::default();
    args.blast.depth = depth;
    args.max_tests = max_tests;
    args.max_impact = max_impact;
    args
}

pub(crate) fn run(args: Args) -> i32 {
    if !build_options().overlay {
        eprintln!(
            "error: --no-overlay does not apply to review: both sides are built with the repo's \
             overlay, and its rules are the working tree's"
        );
        return 2;
    }
    let engine = engine_args(args.depth, args.max_tests, args.max_impact);
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
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_args_carry_the_flags() {
        let a = engine_args(2, 7, 9);
        assert_eq!((a.blast.depth, a.max_tests, a.max_impact), (2, 7, 9));
        let e = ReviewArgs::default();
        let d = engine_args(e.blast.depth, DEFAULT_MAX_TESTS, DEFAULT_MAX_IMPACT);
        assert_eq!(
            (d.blast.depth, d.max_tests, d.max_impact),
            (e.blast.depth, e.max_tests, e.max_impact)
        );
    }
}
