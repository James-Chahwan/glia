//! DE.10: `glia_engine::memory::release_freed_memory` hands freed arena pages
//! back to the OS on Linux glibc and is a no-op elsewhere.
//!
//! One test, in its own binary and so its own process: the RSS it reads is
//! this process's, and the lib test binary, where hundreds of tests allocate in
//! parallel, would make an RSS assertion meaningless. It does not touch the
//! environment: run it without `GLIA_MALLOC_TRIM=0`.

use glia_engine::memory::release_freed_memory;

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn release_freed_memory_returns_freed_arena_pages() {
    const THREADS: usize = 4;
    const BLOCKS: usize = 16 * 1024;
    const BLOCK: usize = 16 * 1024;
    const KEEP_EVERY: usize = 64;

    // Each thread fills its own glibc arena with 16k blocks of 16 KiB (under
    // the mmap threshold, so they live in the arena heaps), written so the
    // pages are resident, then frees all but every 64th. The survivors pin
    // every heap's top, so no heap can shrink on its own: only a trim can
    // return the ~1 GiB in between.
    let kept: Vec<Vec<Vec<u8>>> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..THREADS)
            .map(|t| {
                s.spawn(move || {
                    let mut blocks: Vec<Vec<u8>> = (0..BLOCKS)
                        .map(|i| vec![(t + i) as u8 | 1; BLOCK])
                        .collect();
                    blocks = std::hint::black_box(blocks);
                    blocks
                        .into_iter()
                        .enumerate()
                        .filter(|(i, _)| i % KEEP_EVERY == 0)
                        .map(|(_, b)| b)
                        .collect()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().expect("allocating thread"))
            .collect()
    });
    let kept = std::hint::black_box(kept);
    assert_eq!(
        kept.iter().map(Vec::len).sum::<usize>(),
        THREADS * BLOCKS / KEEP_EVERY
    );

    let trim = release_freed_memory("test").expect("linux-gnu trims and reads /proc/self/status");
    let min_release_kb = 64 * 1024;
    assert!(
        trim.rss_after_kb + min_release_kb <= trim.rss_before_kb,
        "malloc_trim(0) released {} kB of ~1 GiB freed; want >= {min_release_kb} kB ({trim:?})",
        trim.released_kb(),
    );
    assert_eq!(trim.released_kb(), trim.rss_before_kb - trim.rss_after_kb);
    drop(kept);
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
#[test]
fn release_freed_memory_returns_freed_arena_pages() {
    assert_eq!(release_freed_memory("test"), None, "only Linux glibc trims");
}
