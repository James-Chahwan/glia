//! Returns freed heap to the OS after a graph is dropped (glibc `malloc_trim`;
//! a no-op elsewhere): repeated builds in one process level off instead of
//! creeping. Public module slot, reached by module path
//! (`glia_engine::memory::<item>`). Filled by DE.10.
//!
//! Why. A build runs on the engine's rayon pool (`parallel`), and glibc gives
//! each worker thread its own malloc arena. When a graph is dropped its memory
//! goes back to those arenas, fragmented around the few blocks still alive, and
//! glibc only ever shrinks a heap from its top, so the pages stay resident: a
//! process that rebuilds keeps every old graph's footprint. Measured on Linux
//! glibc with repo-graph's rebuild pattern (`generate`, `save_to_default`, swap
//! the live graph; git-archive copies, VmRSS MB first -> last): glia, 40
//! rebuilds, 413 -> 1,335 and still climbing; 15 rebuilds 406 -> 852; grpc-go,
//! 15 rebuilds, 211 -> 487. With `GLIA_THREADS=1` (no workers) it levels off by
//! the fifth rebuild, so the creep is the workers' arenas. `malloc_trim(0)`
//! right after the old graph is freed walks every arena, the workers' included,
//! and hands whole free pages back: glia 15 rebuilds 207 -> 257, 40 rebuilds
//! 208 -> 271 (level from the tenth), grpc-go 107 -> 158, for about 5-10 % more
//! time per rebuild. Trimming at the end of a build instead, before the swap,
//! still creeps (207 -> 383 over 40): the old graph is not freed yet. No
//! allocator crate did better (mimalloc 404 -> 1,051; jemalloc matched only
//! with non-default decay tuning and ran slower), so this is the
//! no-dependency fix.
//!
//! Who calls it. glia-py's `PyGraph` runs [`release_freed_memory`] from a
//! drop guard declared after every other field, so the trim runs once the
//! graph is freed (site `pygraph_drop`); DC's `WatchSession` runs it after it
//! replaces its graph (`watch_update`) and after each cache it evicts for its
//! [`MemoryBudget`] (`watch_budget`, GA.12). An in-process Rust consumer may call it
//! after dropping a `GenerateResult`. A one-shot CLI build does not creep and
//! does not call it.
//!
//! Platforms. Only Linux glibc trims: the creep was measured there and
//! nowhere else. On macOS, Windows and musl this is a no-op that returns
//! `None`; those platforms were not measured. Under an `LD_PRELOAD`ed
//! allocator glibc's `malloc_trim` trims glibc's own, unused arenas: harmless.
//!
//! Opt-out: `GLIA_MALLOC_TRIM=0`. fired_on, on stderr, once per trim:
//! `[memory] malloc_trim site=<site> rss_mb=<before>-><after>` (`?` for a
//! reading `/proc/self/status` did not give).
//!
//! One memory budget (0.5.6, GA.12): [`MemoryBudget`] is the one value that
//! bounds what a long-lived holder keeps beyond its graph, read from
//! [`BUDGET_ENV`] (`GLIA_MEMORY_BUDGET_MB`) or given explicitly (`glia watch
//! --memory-budget-mb`, pyo3 `WatchSession(memory_budget_mb=)`). The same
//! value bounds a watch session's caches (its cached language group builds
//! and DC.4 build memos, evicted past it: `watch::WatchSession`) and a rules
//! evaluation's working set (FA.14). It is compared with the PROCESS's
//! resident set ([`rss_kb`]), not a session's own size: a host process (an
//! MCP server, a `PyGraph` held beside the session, an embedder) counts too,
//! so the budget is a process target. A budget below what the process needs
//! with every cache dropped (one graph, its parse cache, the host) evicts
//! every cache at every update and costs time, never bytes: an evicted cache
//! is rebuilt by the next update exactly as a miss computes it. fired_on, on
//! stderr: `[memory] budget ignored value=<v>` once per unparseable
//! `GLIA_MEMORY_BUDGET_MB` read.

use std::io::Write as _;

/// The environment variable that turns the trim off: `0` disables it, unset
/// or any other value leaves it on.
pub const TRIM_ENV: &str = "GLIA_MALLOC_TRIM";

/// Whether this build of the engine can trim at all: Linux glibc only.
const TRIMS: bool = cfg!(all(target_os = "linux", target_env = "gnu"));

/// GA.12: the environment variable [`MemoryBudget::from_env`] reads, in MB:
/// unset, empty or `0` is no budget; anything that is not a whole number is
/// no budget plus one `[memory] budget ignored value=<v>` line.
pub const BUDGET_ENV: &str = "GLIA_MEMORY_BUDGET_MB";

/// GA.12: the one memory budget (module doc): a resident-set target in MB
/// for the whole process, or none. `Default` is none.
///
/// A holder compares it with [`rss_kb`] after freeing what it can
/// ([`release_freed_memory`]) and drops caches while [`Self::exceeded`]
/// holds; it never drops anything without a reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemoryBudget {
    mb: Option<u64>,
}

impl MemoryBudget {
    /// No budget: nothing is ever evicted for memory.
    pub fn none() -> Self {
        Self::default()
    }

    /// A budget of `n` MB of process RSS; `0` is no budget, as in
    /// [`BUDGET_ENV`].
    pub fn mb(n: u64) -> Self {
        Self {
            mb: (n > 0).then_some(n),
        }
    }

    /// The budget [`BUDGET_ENV`] sets (see there); none when it is unset.
    pub fn from_env() -> Self {
        Self::from_value(std::env::var(BUDGET_ENV).ok().as_deref())
    }

    /// [`Self::from_env`]'s reading of the variable's value.
    fn from_value(value: Option<&str>) -> Self {
        let Some(raw) = value else {
            return Self::none();
        };
        let v = raw.trim();
        if v.is_empty() {
            return Self::none();
        }
        match v.parse::<u64>() {
            Ok(n) => Self::mb(n),
            Err(_) => {
                // Not `eprintln!`: a closed stderr must not panic a holder.
                let _ = writeln!(std::io::stderr(), "[memory] budget ignored value={v}");
                Self::none()
            }
        }
    }

    /// The budget in MB, `None` for no budget.
    pub fn budget_mb(&self) -> Option<u64> {
        self.mb
    }

    /// The budget in kB (the unit [`rss_kb`] reads), `None` for no budget.
    pub fn limit_kb(&self) -> Option<u64> {
        self.mb.map(|mb| mb.saturating_mul(1024))
    }

    /// Whether a resident set of `rss_kb` kB is over the budget (never
    /// without one).
    pub fn exceeded(&self, rss_kb: u64) -> bool {
        self.limit_kb().is_some_and(|limit| rss_kb > limit)
    }
}

/// One trim's effect: the process's resident set (`VmRSS`, kB) read just
/// before and just after `malloc_trim(0)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Trim {
    pub rss_before_kb: u64,
    pub rss_after_kb: u64,
}

impl Trim {
    /// Resident kB the trim handed back (0 when RSS grew meanwhile, e.g.
    /// another thread allocating).
    pub fn released_kb(&self) -> u64 {
        self.rss_before_kb.saturating_sub(self.rss_after_kb)
    }
}

/// Hand the heap the caller just freed back to the OS, logging the marker.
/// `site` names the caller in the marker (`pygraph_drop`, `watch_update`).
///
/// `Some` on Linux glibc when the trim ran and both RSS readings succeeded;
/// `None` when `GLIA_MALLOC_TRIM=0`, off glibc, or when `/proc/self/status`
/// could not be read (the trim still ran then). Never panics; cost is one walk
/// of every arena plus two small `/proc` reads.
pub fn release_freed_memory(site: &str) -> Option<Trim> {
    if !TRIMS || !trim_enabled(std::env::var(TRIM_ENV).ok().as_deref()) {
        return None;
    }
    let before = rss_kb();
    sys::malloc_trim_all();
    let after = rss_kb();
    let mb = |kb: Option<u64>| kb.map_or_else(|| "?".to_string(), |k| (k / 1024).to_string());
    // Not `eprintln!`, which panics when stderr is a closed pipe: this runs in
    // `Drop` impls, where a panic must not happen.
    let _ = writeln!(
        std::io::stderr(),
        "[memory] malloc_trim site={site} rss_mb={}->{}",
        mb(before),
        mb(after)
    );
    Some(Trim {
        rss_before_kb: before?,
        rss_after_kb: after?,
    })
}

/// `GLIA_MALLOC_TRIM`'s reading: unset or anything but `0` is on.
fn trim_enabled(env: Option<&str>) -> bool {
    env.map(str::trim) != Some("0")
}

/// This process's resident set in kB, from `/proc/self/status`; `None` where
/// there is no procfs. Public since GA.12: the budget's reading, and the one
/// a holder's RSS series is printed from.
pub fn rss_kb() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .as_deref()
        .and_then(parse_vm_rss)
}

/// The kB value of the `VmRSS:` line of a `/proc/<pid>/status` text.
fn parse_vm_rss(status_text: &str) -> Option<u64> {
    let rest = status_text.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
    let mut words = rest.split_whitespace();
    let kb = words.next()?.parse().ok()?;
    (words.next() == Some("kB")).then_some(kb)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod sys {
    // glibc exports `malloc_trim` from libc.so.6, which every Rust binary on
    // linux-gnu links already: no crate, no Cargo.lock change.
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> std::os::raw::c_int;
    }

    /// `malloc_trim(0)`: every arena's free pages back to the OS. Its return
    /// (1 when memory was released) is not needed: the RSS readings say it.
    pub(super) fn malloc_trim_all() {
        // SAFETY: malloc_trim takes a size and touches only the allocator's
        // own state, under the allocator's own locks; it frees no live block
        // and is safe to call from any thread at any time.
        unsafe {
            malloc_trim(0);
        }
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
mod sys {
    /// No trim off glibc (see the module doc); `TRIMS` keeps it uncalled.
    pub(super) fn malloc_trim_all() {}
}

#[cfg(test)]
mod tests {
    use super::{MemoryBudget, Trim, parse_vm_rss, trim_enabled};

    #[test]
    fn budget_reads_unset_empty_and_zero_as_none() {
        for v in [None, Some(""), Some("  "), Some("0"), Some(" 0\n")] {
            assert_eq!(MemoryBudget::from_value(v), MemoryBudget::none(), "{v:?}");
        }
        assert_eq!(MemoryBudget::from_value(Some("512")), MemoryBudget::mb(512));
        assert_eq!(
            MemoryBudget::from_value(Some(" 2048\n")).budget_mb(),
            Some(2048)
        );
        assert_eq!(MemoryBudget::mb(0), MemoryBudget::default());
    }

    #[test]
    fn unparseable_budget_is_none() {
        for v in ["lots", "-1", "1.5", "512MB"] {
            assert_eq!(
                MemoryBudget::from_value(Some(v)),
                MemoryBudget::none(),
                "{v}"
            );
        }
    }

    #[test]
    fn budget_limit_and_exceeded() {
        let b = MemoryBudget::mb(2);
        assert_eq!(b.limit_kb(), Some(2048));
        assert!(!b.exceeded(2048), "at the budget is within it");
        assert!(b.exceeded(2049));
        let none = MemoryBudget::none();
        assert_eq!(none.limit_kb(), None);
        assert!(!none.exceeded(u64::MAX), "no budget is never exceeded");
        assert_eq!(MemoryBudget::mb(u64::MAX).limit_kb(), Some(u64::MAX));
    }

    #[test]
    fn trim_is_on_unless_the_env_says_0() {
        assert!(trim_enabled(None));
        assert!(trim_enabled(Some("1")));
        assert!(trim_enabled(Some("")));
        assert!(!trim_enabled(Some("0")));
        assert!(!trim_enabled(Some(" 0\n")));
    }

    #[test]
    fn vm_rss_is_read_from_its_line_in_kb() {
        let status = "Name:\tglia\nVmPeak:\t  900000 kB\nVmHWM:\t  420000 kB\nVmRSS:\t  271360 kB\nThreads:\t17\n";
        assert_eq!(parse_vm_rss(status), Some(271_360));
        assert_eq!(
            parse_vm_rss("Name:\tglia\nThreads:\t1\n"),
            None,
            "no VmRSS line"
        );
        assert_eq!(
            parse_vm_rss("VmRSS:\t  12 MB\n"),
            None,
            "a unit other than kB"
        );
        assert_eq!(parse_vm_rss("VmRSS:\t  lots kB\n"), None, "not a number");
        assert_eq!(parse_vm_rss("VmRSS:\n"), None, "empty value");
    }

    #[test]
    fn released_is_the_drop_and_never_negative() {
        let t = Trim {
            rss_before_kb: 400_000,
            rss_after_kb: 250_000,
        };
        assert_eq!(t.released_kb(), 150_000);
        let grew = Trim {
            rss_before_kb: 1_000,
            rss_after_kb: 1_200,
        };
        assert_eq!(grew.released_kb(), 0);
    }
}
