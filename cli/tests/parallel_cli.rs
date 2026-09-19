//! LG.1a: `glia analyze` routes a repo's files on the engine's rayon pool,
//! sized by `GLIA_THREADS`, and the `[parallel]` stderr line is its fired_on
//! marker. The pool size never reaches the output: the JSON document of a
//! 4-thread run and of a `GLIA_THREADS=1` run (no pool: the caller's thread,
//! the pre-LG.1a path) are byte-equal.
//!
//! LG.1b: the walk's file reads, the const-table scan and the RPC needle pass
//! run on the same pool, each with its own `[parallel]` line: one
//! `[parallel] walk <root>: read ...` per walk and one
//! `[parallel] <repo>: const-scan ...` per repo build.

use std::path::PathBuf;
use std::process::Output;

fn fixture(name: &str) -> String {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent()
        .expect("cli/ has a parent")
        .join("tests/fixtures")
        .join(name)
        .to_str()
        .expect("fixture path is UTF-8")
        .to_string()
}

fn analyze(threads: &str) -> Output {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["analyze", &fixture("py_smoke"), "--format", "json"])
        .env("GLIA_THREADS", threads)
        .output()
        .expect("glia runs");
    assert!(
        out.status.success(),
        "glia analyze (GLIA_THREADS={threads}) exited {:?}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// The one `[parallel] ` line of a single-repo run that contains `needle`,
/// relayed so `-- --nocapture | grep '^\[parallel\] '` sees it.
fn parallel_line(out: &Output, needle: &str) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[parallel] ") && l.contains(needle))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "one [parallel] line holding {needle:?} per repo:\n{stderr}"
    );
    eprintln!("{}", lines[0]);
    lines[0].to_string()
}

#[test]
fn analyze_reports_parallel_routing() {
    let four = analyze("4");
    let line = parallel_line(&four, ": routed ");
    assert!(line.contains("on 4 threads"), "{line}");
    assert!(line.contains("failed 0)"), "{line}");
    // LG.1b: the walk reads on the pool; py_smoke is all Python source.
    let walk = parallel_line(&four, "[parallel] walk ");
    assert!(walk.contains("py_smoke: read "), "{walk}");
    assert!(
        walk.contains(" files on 4 threads (md 0, json 0, source "),
        "{walk}"
    );
    // LG.1b: the const-table scan and the RPC needle pass, once per repo.
    let consts = parallel_line(&four, ": const-scan ");
    assert!(
        consts.contains(" files, rpc-needles 0 files on 4 threads"),
        "{consts}"
    );

    let one = analyze("1");
    let line = parallel_line(&one, ": routed ");
    assert!(line.contains("on 1 threads"), "{line}");
    assert!(parallel_line(&one, "[parallel] walk ").contains(" files on 1 threads ("));
    assert!(parallel_line(&one, ": const-scan ").contains(" files on 1 threads"));

    assert!(!four.stdout.is_empty());
    assert!(
        four.stdout == one.stdout,
        "the analyze JSON must not depend on the pool size"
    );
}
