//! Per-phase build timers (CA.9): what each phase of a build costs, printed
//! as `[timing]` stderr lines and nowhere else. A time never reaches a
//! `FileParse`, the parse cache, a `.gmap`, the manifest, stdout or an
//! answer, so every stored and printed output stays byte-identical.
//!
//! Every timer is the wall time of one phase call on the orchestrating
//! thread: a phase that fans out on the engine pool counts what a user waits
//! for, not the CPU its workers spent.
//!
//! fired_on markers, on stderr:
//! - `[timing] repo=<label> walk=<ms> parse=<ms> const_scan=<ms> grafts=<ms> language_build=<ms>`
//!   once per built repo, after its graphs are built (a multi-repo build
//!   prints them in argument order). `walk` is `walk_source_files`; `parse`
//!   the per-file route / parse / extract (cache lookups included);
//!   `const_scan` the A11.1 const table and its LF.2d overlay pins; `grafts`
//!   the Cargo-package read and `grafts::apply_post_cache`; `language_build`
//!   the per-language graph builds.
//! - `[timing] build repos=<n> resolve=<ms> post=<ms> finalize=<ms> external_cells=<ms> total=<ms> slowest_pass=<name>:<ms>`
//!   once per build, from [`crate::build`]'s `generate_*` (`total` = the
//!   whole call) and from a layout merge (`crate::merge::merge_layouts`,
//!   `total` = the merge, loads included), which runs no external-cell stage
//!   and so prints no `external_cells=`. `slowest_pass=none` when no pass ran.
//! - `[timing] persist writer=<w> <ms> dir=<dir>` once per layout write
//!   (`persist::persist_layout`, `persist::persist_graph` with its orphan
//!   sweep, `merge::persist_merge`).
//!
//! `<ms>` is milliseconds with one decimal, TRUNCATED (`12.34` ms prints
//! `12.3ms`): parts that sum to at most a total still print a sum no larger
//! than the printed total.

use std::path::Path;
use std::time::Duration;

use glia_activation::passes::{PassReport, Stage};

/// One repo's build phases (the `[timing] repo=` line). `walk` is filled by
/// the caller that ran the walk; `assemble::build_graphs_for_repo` times the
/// other four.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PhaseTimes {
    pub(crate) walk: Duration,
    pub(crate) parse: Duration,
    pub(crate) const_scan: Duration,
    pub(crate) grafts: Duration,
    pub(crate) language_build: Duration,
}

impl PhaseTimes {
    /// `[timing] repo=<label> walk=<ms> parse=<ms> const_scan=<ms> grafts=<ms> language_build=<ms>`
    pub(crate) fn repo_marker(&self, label: &str) -> String {
        format!(
            "[timing] repo={label} walk={} parse={} const_scan={} grafts={} language_build={}",
            ms(self.walk),
            ms(self.parse),
            ms(self.const_scan),
            ms(self.grafts),
            ms(self.language_build),
        )
    }
}

/// One build's pass tail and total (the `[timing] build` line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BuildTimes {
    pub(crate) repos: usize,
    pub(crate) resolve: Duration,
    pub(crate) post: Duration,
    pub(crate) finalize: Duration,
    /// `None` for a build with no external-cell stage (a layout merge).
    pub(crate) external_cells: Option<Duration>,
    pub(crate) total: Duration,
    pub(crate) slowest: Option<(&'static str, Duration)>,
}

impl BuildTimes {
    /// The stage times and slowest pass of `report`, with the rest.
    pub(crate) fn new(
        repos: usize,
        report: &PassReport,
        external_cells: Option<Duration>,
        total: Duration,
    ) -> Self {
        Self {
            repos,
            resolve: report.stage_elapsed(Stage::Resolve),
            post: report.stage_elapsed(Stage::Post),
            finalize: report.stage_elapsed(Stage::Finalize),
            external_cells,
            total,
            slowest: report.slowest(),
        }
    }

    /// `[timing] build repos=<n> resolve=<ms> post=<ms> finalize=<ms> [external_cells=<ms> ]total=<ms> slowest_pass=<name>:<ms>`
    pub(crate) fn marker(&self) -> String {
        let external = self
            .external_cells
            .map(|d| format!(" external_cells={}", ms(d)))
            .unwrap_or_default();
        let slowest = match self.slowest {
            Some((name, d)) => format!("{name}:{}", ms(d)),
            None => "none".to_string(),
        };
        format!(
            "[timing] build repos={} resolve={} post={} finalize={}{external} total={} slowest_pass={slowest}",
            self.repos,
            ms(self.resolve),
            ms(self.post),
            ms(self.finalize),
            ms(self.total),
        )
    }
}

/// `[timing] persist writer=<w> <ms> dir=<dir>`
pub(crate) fn persist_marker(writer: &str, took: Duration, dir: &Path) -> String {
    format!("[timing] persist writer={writer} {} dir={}", ms(took), dir.display())
}

/// `d` as milliseconds with one decimal, truncated: `12.3ms`.
pub(crate) fn ms(d: Duration) -> String {
    let tenths = d.as_micros() / 100;
    format!("{}.{}ms", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn us(n: u64) -> Duration {
        Duration::from_micros(n)
    }

    #[test]
    fn ms_truncates_to_one_decimal() {
        assert_eq!(ms(Duration::ZERO), "0.0ms");
        assert_eq!(ms(us(99)), "0.0ms");
        assert_eq!(ms(us(12_349)), "12.3ms");
        assert_eq!(ms(us(12_399)), "12.3ms");
        assert_eq!(ms(Duration::from_secs(3)), "3000.0ms");
    }

    #[test]
    fn markers_are_stable() {
        let phases = PhaseTimes {
            walk: us(1_250),
            parse: us(40_000),
            const_scan: us(3_070),
            grafts: us(900),
            language_build: us(15_555),
        };
        assert_eq!(
            phases.repo_marker("tests/fixtures/py_smoke"),
            "[timing] repo=tests/fixtures/py_smoke walk=1.2ms parse=40.0ms const_scan=3.0ms \
             grafts=0.9ms language_build=15.5ms"
        );

        let build = BuildTimes {
            repos: 2,
            resolve: us(8_000),
            post: us(2_500),
            finalize: us(1_000),
            external_cells: Some(us(120)),
            total: us(90_000),
            slowest: Some(("http", us(6_789))),
        };
        assert_eq!(
            build.marker(),
            "[timing] build repos=2 resolve=8.0ms post=2.5ms finalize=1.0ms external_cells=0.1ms \
             total=90.0ms slowest_pass=http:6.7ms"
        );
        let merge = BuildTimes { external_cells: None, slowest: None, ..build };
        assert_eq!(
            merge.marker(),
            "[timing] build repos=2 resolve=8.0ms post=2.5ms finalize=1.0ms total=90.0ms \
             slowest_pass=none"
        );

        assert_eq!(
            persist_marker("cli", us(4_321), Path::new("/r/.glia/graph")),
            "[timing] persist writer=cli 4.3ms dir=/r/.glia/graph"
        );
    }

    #[test]
    fn build_times_read_the_pass_report() {
        let mut report = PassReport::default();
        report.elapsed = [us(5_000), us(2_000), us(1_000)];
        report.pass_elapsed = vec![("grpc", us(1_000)), ("http", us(4_000)), ("sort", us(1_000))];
        let t = BuildTimes::new(1, &report, Some(us(300)), us(20_000));
        assert_eq!((t.resolve, t.post, t.finalize), (us(5_000), us(2_000), us(1_000)));
        assert_eq!(t.slowest, Some(("http", us(4_000))));
        assert_eq!(t.external_cells, Some(us(300)));
        assert!(t.marker().ends_with(" total=20.0ms slowest_pass=http:4.0ms"), "{}", t.marker());
    }
}
