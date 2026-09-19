//! LG.1a: `glia analyze` routes a repo's files on the engine's rayon pool,
//! sized by `GLIA_THREADS`, and the `[parallel]` stderr line is its fired_on
//! marker. The pool size never reaches the output: the JSON document of a
//! 4-thread run and of a `GLIA_THREADS=1` run (no pool: the caller's thread,
//! the pre-LG.1a path) are byte-equal.

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

/// The one `[parallel] ` line of a single-repo run, relayed so
/// `-- --nocapture | grep '^\[parallel\] '` sees it.
fn parallel_line(out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[parallel] "))
        .collect();
    assert_eq!(lines.len(), 1, "one [parallel] line per repo:\n{stderr}");
    eprintln!("{}", lines[0]);
    lines[0].to_string()
}

#[test]
fn analyze_reports_parallel_routing() {
    let four = analyze("4");
    let line = parallel_line(&four);
    assert!(line.contains("on 4 threads"), "{line}");
    assert!(line.contains(": routed "), "{line}");
    assert!(line.contains("failed 0)"), "{line}");

    let one = analyze("1");
    let line = parallel_line(&one);
    assert!(line.contains("on 1 threads"), "{line}");

    assert!(!four.stdout.is_empty());
    assert!(
        four.stdout == one.stdout,
        "the analyze JSON must not depend on the pool size"
    );
}
