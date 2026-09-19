//! The engine's own rayon pool for the per-file route / parse / extract
//! (LG.1a), thread-safe quiet panics, and the order-preserving map that keeps
//! a parallel build byte-identical to a sequential one. Extended by LG.1b.
//!
//! Why a dedicated pool and not rayon's global one: the worker stack size is
//! ours to set (see [`WORKER_STACK`]), and an embedding app that uses the
//! global pool (neuropil) never shares queues with a build.
//!
//! Determinism: [`par_map_ordered`] collects through an indexed parallel
//! iterator, so result `i` is item `i`'s whatever thread ran it. Callers fold
//! the results sequentially, in input order, and never push into shared state
//! from inside the mapped closure.
//!
//! Quiet panics: one process-wide panic hook, installed once and never
//! swapped back, forwards to the hook it replaced unless the panicking thread
//! is inside a [`quiet`] call (or a [`quiet_scope`]). The flag is thread-local,
//! so a panic on any other thread of the process (another build, the
//! embedding app) still reaches the original hook. This replaces the old
//! `SuppressPanicHook`, which swapped the global hook for a no-op for the
//! whole build and so silenced every thread.
//!
//! Crate-private: cross-module items are `pub(crate)`.

use std::cell::Cell;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Once, OnceLock};

use rayon::prelude::*;

/// Upper bound on `GLIA_THREADS`.
const MAX_THREADS: usize = 256;

/// Worker stack size. Every language parser recurses per AST level, rayon's
/// default worker stack is 2 MiB, and a stack overflow aborts the process
/// (`catch_unwind` cannot catch it). The main thread, and a CPython caller's,
/// runs with 8 MiB; 16 MiB gives headroom over that. It is a virtual
/// reservation only: 16 workers reserve 256 MiB of address space, resident
/// memory is unchanged.
const WORKER_STACK: usize = 16 << 20;

static THREADS: OnceLock<usize> = OnceLock::new();
static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
static HOOK: Once = Once::new();

thread_local! {
    /// Is this thread inside [`quiet`] / [`quiet_scope`]? Read by the hook.
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// The engine pool's size: `GLIA_THREADS`, read once per process. Unset, `0`
/// or unparsable means every core ([`std::thread::available_parallelism`],
/// 1 when unknown); the value is clamped to `1..=256`. `GLIA_THREADS=1` builds
/// no pool at all: the per-file work runs on the caller's thread, exactly as
/// before LG.1a.
pub(crate) fn threads() -> usize {
    *THREADS.get_or_init(|| {
        let available = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        threads_from(std::env::var("GLIA_THREADS").ok().as_deref(), available)
    })
}

/// [`threads`] without the environment: `var` is `GLIA_THREADS`' value.
fn threads_from(var: Option<&str>, available: usize) -> usize {
    let n = match var.and_then(|v| v.trim().parse::<usize>().ok()) {
        Some(n) if n > 0 => n,
        _ => available,
    };
    n.clamp(1, MAX_THREADS)
}

/// A pool of `n` big-stack workers named `glia-parse-<i>`. `None` when the
/// OS refuses the threads: the caller degrades to sequential, never panics.
fn build_pool(n: usize) -> Option<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .stack_size(WORKER_STACK)
        .thread_name(|i| format!("glia-parse-{i}"))
        .build()
        .ok()
}

/// The process's engine pool, built on first use; `None` when
/// [`threads`] is 1 or the pool could not be built.
fn engine_pool() -> Option<&'static rayon::ThreadPool> {
    POOL.get_or_init(|| (threads() > 1).then(|| build_pool(threads())).flatten())
        .as_ref()
}

/// `items.map(f)` in input order, on a pool, plus the thread count that ran
/// it (for the caller's fired_on marker).
///
/// - Already on a rayon worker (a caller's own pool: a test, an embedding
///   app, LG.1b's repo-level map on the engine pool): run there, so a nested
///   call never blocks a worker on a second pool.
/// - Otherwise on the engine pool.
/// - With no engine pool (`GLIA_THREADS=1`): sequentially on this thread.
pub(crate) fn par_map_ordered<T: Sync, R: Send>(
    items: &[T],
    f: impl Fn(&T) -> R + Sync + Send,
) -> (Vec<R>, usize) {
    if rayon::current_thread_index().is_some() {
        return (
            items.par_iter().map(f).collect(),
            rayon::current_num_threads(),
        );
    }
    match engine_pool() {
        Some(pool) => (
            pool.install(|| items.par_iter().map(&f).collect()),
            pool.current_num_threads(),
        ),
        None => (items.iter().map(f).collect(), 1),
    }
}

/// Install the quiet-aware panic hook, once per process. It wraps whatever
/// hook was current then and calls it for every panic on a thread that is
/// not inside [`quiet`] / [`quiet_scope`].
fn install_quiet_hook() {
    HOOK.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // `try_with`: a panic during thread-local teardown still reports.
            if !QUIET.try_with(Cell::get).unwrap_or(false) {
                prev(info);
            }
        }));
    });
}

/// RAII: marks this thread quiet and restores the previous flag on drop,
/// also on unwind. Not `Send`: the flag it restores is this thread's.
pub(crate) struct QuietGuard {
    prev: bool,
    _not_send: PhantomData<*const ()>,
}

impl QuietGuard {
    fn enter() -> Self {
        install_quiet_hook();
        Self {
            prev: QUIET.with(|q| q.replace(true)),
            _not_send: PhantomData,
        }
    }
}

impl Drop for QuietGuard {
    fn drop(&mut self) {
        let prev = self.prev;
        let _ = QUIET.try_with(|q| q.set(prev));
    }
}

/// Keep panics on THIS thread off stderr until the guard drops. The build
/// holds one for a whole repo assembly, so every `catch_unwind` it runs on
/// the calling thread stays silent; work on pool threads goes through
/// [`quiet`] instead.
pub(crate) fn quiet_scope() -> QuietGuard {
    QuietGuard::enter()
}

/// Run `f` with this thread quiet and catch its panic: a caught panic is an
/// `Err` carrying the payload and prints nothing. Safe on any thread, any
/// number at once.
pub(crate) fn quiet<R>(f: impl FnOnce() -> R) -> std::thread::Result<R> {
    let _guard = QuietGuard::enter();
    catch_unwind(AssertUnwindSafe(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glia_threads_parses_and_clamps() {
        assert_eq!(threads_from(None, 16), 16);
        assert_eq!(threads_from(Some("0"), 16), 16, "0 means every core");
        assert_eq!(
            threads_from(Some("lots"), 16),
            16,
            "unparsable means every core"
        );
        assert_eq!(threads_from(Some(" 4 "), 16), 4);
        assert_eq!(threads_from(Some("1"), 16), 1);
        assert_eq!(threads_from(Some("100000"), 16), MAX_THREADS);
        assert_eq!(threads_from(None, 0), 1, "an unknown core count still runs");
    }

    #[test]
    fn a_panicking_file_is_an_error_not_an_abort() {
        let pool = build_pool(8).expect("an 8-thread pool");
        // `broadcast` runs the closure once on every worker.
        let caught = pool.broadcast(|_| quiet(|| panic!("glia-quiet-probe")).is_err());
        assert_eq!(caught, vec![true; 8], "every worker caught its panic");
        let flags = pool.broadcast(|_| QUIET.with(Cell::get));
        assert_eq!(
            flags,
            vec![false; 8],
            "the quiet flag is restored on every worker"
        );

        // Nested: an inner `quiet` restores the outer scope's flag.
        let _scope = quiet_scope();
        assert!(QUIET.with(Cell::get));
        assert!(quiet(|| panic!("glia-quiet-probe")).is_err());
        assert!(QUIET.with(Cell::get), "still inside the outer scope");
        drop(_scope);
        assert!(!QUIET.with(Cell::get));
    }

    #[test]
    fn par_map_ordered_keeps_input_order() {
        let pool = build_pool(4).expect("a 4-thread pool");
        let items: Vec<u64> = (0..10_000).collect();
        let (out, threads) = pool.install(|| {
            par_map_ordered(&items, |x| {
                let start = std::time::Instant::now();
                while start.elapsed() < std::time::Duration::from_micros(20) {
                    std::hint::spin_loop();
                }
                x * 2
            })
        });
        assert_eq!(threads, 4);
        assert_eq!(out, items.iter().map(|x| x * 2).collect::<Vec<_>>());
    }

    /// Recurse until this thread's stack is `bytes` below `base` (the
    /// address of a local in the caller): the depth is measured, not assumed,
    /// so it holds whatever frame size the build profile gives. 64 KiB frames,
    /// read after the call returns so the recursion is not a tail call.
    fn burn(base: usize, bytes: usize) -> usize {
        let frame = std::hint::black_box([0u8; 65_536]);
        let here = std::ptr::addr_of!(frame) as usize;
        if base.saturating_sub(here) >= bytes {
            return usize::from(frame[0]);
        }
        let below = burn(base, bytes);
        usize::from(std::hint::black_box(&frame)[1]) + below
    }

    /// Stack-top address of the calling thread (a local's address).
    fn stack_base() -> usize {
        let marker = std::hint::black_box(0u8);
        std::ptr::addr_of!(marker) as usize
    }

    #[test]
    fn engine_pool_workers_have_16_mib_stacks() {
        let pool = build_pool(4).expect("a 4-thread pool");
        let out = pool.install(|| {
            (0..8)
                .into_par_iter()
                .map(|_| burn(stack_base(), 12 << 20))
                .collect::<Vec<_>>()
        });
        assert_eq!(out, vec![0; 8]);
    }
}
