//! CA.9: every build prints what each phase cost, on stderr only.
//!
//! - `[timing] repo=<label> walk=<ms> parse=<ms> const_scan=<ms> grafts=<ms> language_build=<ms>`
//!   once per built repo, in argument order;
//! - `[timing] build repos=<n> resolve=<ms> post=<ms> finalize=<ms> [external_cells=<ms>] total=<ms> slowest_pass=<name>:<ms>`
//!   once per build (a layout merge, which runs no external-cell stage, has
//!   no `external_cells=`);
//! - `[timing] persist writer=<w> <ms> dir=<dir>` once per layout write.
//!
//! Each `<ms>` is milliseconds with one decimal, truncated (`12.3ms`), so the
//! printed parts of a build never sum past its printed total. Wall-clock
//! numbers vary run to run, so they must never reach stdout: a second run's
//! stdout is byte-equal. The `[timing]` lines are relayed, so the fired_on
//! marker is grep-able:
//! `cargo test -p glia-cli --test timing_cli -- --nocapture 2>&1 | grep -o '\[timing\] .*'`
//! (`-o`, not `^`: the harness's `test <name> ... ` prefix shares a line with
//! the first relayed one).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli/ has a parent")
        .join("tests/fixtures")
        .join(name)
}

fn fixture_str(name: &str) -> String {
    fixture(name).to_str().expect("fixture path is UTF-8").to_string()
}

/// A scratch dir, created fresh and removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`).
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-ca9-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir.canonicalize().expect("canonical scratch dir"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).expect("copy fixture file");
        }
    }
}

/// `glia <args>`, asserted successful, `[timing] ` lines relayed.
fn glia(args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env_remove("GLIA_NO_PERSIST")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "glia {args:?} exited {:?}\nstderr:\n{stderr}", out.status);
    for line in stderr.lines() {
        if line.starts_with("[timing] ") {
            eprintln!("{line}");
        }
    }
    out
}

/// The stderr lines of `out` that start with `prefix`.
fn lines(out: &Output, prefix: &str) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with(prefix))
        .map(String::from)
        .collect()
}

/// The one line of `out` starting with `prefix`.
fn one_line(out: &Output, prefix: &str) -> String {
    let found = lines(out, prefix);
    assert_eq!(
        found.len(),
        1,
        "exactly one {prefix:?} line:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    found[0].clone()
}

/// The milliseconds of `key=<n>ms` in `line`, or `None` when the key is absent.
fn ms_of(line: &str, key: &str) -> Option<f64> {
    let at = line.find(&format!(" {key}="))?;
    let value = &line[at + key.len() + 2..];
    let value = value.split(' ').next().unwrap_or_default();
    let n = value.strip_suffix("ms").unwrap_or_else(|| panic!("{key} is <n>ms: {line}"));
    Some(n.parse().unwrap_or_else(|_| panic!("{key} is a number: {line}")))
}

fn ms(line: &str, key: &str) -> f64 {
    ms_of(line, key).unwrap_or_else(|| panic!("{key}= in {line}"))
}

const REPO_PHASES: [&str; 5] = ["walk", "parse", "const_scan", "grafts", "language_build"];

/// A `[timing] repo=` line names all five phases, and they fit in `total`.
fn check_repo_line(line: &str, total: f64) {
    let sum: f64 = REPO_PHASES.iter().map(|k| ms(line, k)).sum();
    assert!(sum <= total + 1e-9, "repo phases {sum} > build total {total}: {line}");
}

/// A `[timing] build` line: its stage parts fit in its total, and it names
/// the slowest pass. Returns the total.
fn check_build_line(line: &str, external_cells: bool) -> f64 {
    let total = ms(line, "total");
    let ext = ms_of(line, "external_cells");
    assert_eq!(ext.is_some(), external_cells, "external_cells= present: {line}");
    let parts = ms(line, "resolve") + ms(line, "post") + ms(line, "finalize") + ext.unwrap_or(0.0);
    assert!(parts <= total + 1e-9, "stages {parts} > total {total}: {line}");
    let slowest = line.split(" slowest_pass=").nth(1).expect("slowest_pass=");
    let (name, time) = slowest.rsplit_once(':').expect("slowest_pass=<name>:<ms>");
    assert!(!name.is_empty() && name != "none", "a code build runs passes: {line}");
    assert!(time.ends_with("ms"), "{line}");
    total
}

#[test]
fn analyze_times_the_repo_and_the_build() {
    let repo = fixture_str("py_smoke");
    let first = glia(&["analyze", &repo, "--format", "json"]);
    let repo_line = one_line(&first, "[timing] repo=");
    assert!(repo_line.starts_with(&format!("[timing] repo={repo} walk=")), "{repo_line}");
    let build = one_line(&first, "[timing] build ");
    assert!(build.starts_with("[timing] build repos=1 resolve="), "{build}");
    let total = check_build_line(&build, true);
    check_repo_line(&repo_line, total);
    // analyze writes no layout.
    assert!(lines(&first, "[timing] persist ").is_empty());

    let second = glia(&["analyze", &repo, "--format", "json"]);
    assert!(!first.stdout.is_empty());
    assert!(first.stdout == second.stdout, "timings never reach stdout");
}

#[test]
fn a_source_merge_times_each_repo_in_argument_order() {
    let (py, go) = (fixture_str("py_smoke"), fixture_str("go_smoke"));
    let out = glia(&["merge", &py, &go]);
    let repos = lines(&out, "[timing] repo=");
    assert_eq!(repos.len(), 2, "{repos:?}");
    assert!(repos[0].starts_with(&format!("[timing] repo={py} walk=")), "{repos:?}");
    assert!(repos[1].starts_with(&format!("[timing] repo={go} walk=")), "{repos:?}");
    let build = one_line(&out, "[timing] build ");
    assert!(build.starts_with("[timing] build repos=2 "), "{build}");
    let total = check_build_line(&build, true);
    for r in &repos {
        check_repo_line(r, total);
    }
}

#[test]
fn build_times_the_persist_and_a_layout_merge_times_its_passes() {
    let s = Scratch::new("persist");
    let (a, b) = (s.0.join("a"), s.0.join("b"));
    copy_tree(&fixture("py_smoke"), &a);
    copy_tree(&fixture("go_smoke"), &b);
    let a_str = a.to_str().expect("UTF-8 scratch path");
    let built = glia(&["build", a_str]);
    let persist = one_line(&built, "[timing] persist ");
    let dir = a.join(".glia/graph");
    assert!(persist.starts_with("[timing] persist writer=cli "), "{persist}");
    assert!(persist.ends_with(&format!(" dir={}", dir.display())), "{persist}");
    let elapsed = persist.split(' ').nth(3).expect("<ms> field");
    assert!(elapsed.ends_with("ms"), "{persist}");
    elapsed.trim_end_matches("ms").parse::<f64>().expect("<ms> is a number");
    one_line(&built, "[timing] repo=");
    one_line(&built, "[timing] build repos=1 ");
    glia(&["build", b.to_str().expect("UTF-8 scratch path")]);

    // A layout merge loads both layouts (no walk, no parse: no repo line) and
    // re-runs the passes over the union; it has no external-cell stage.
    let gmap_a = dir.to_str().expect("UTF-8").to_string();
    let gmap_b = b.join(".glia/graph").to_str().expect("UTF-8").to_string();
    let out_dir = s.0.join("merged");
    let merged = glia(&[
        "merge",
        "--gmap",
        &gmap_a,
        "--gmap",
        &gmap_b,
        "--layout",
        out_dir.to_str().expect("UTF-8"),
    ]);
    assert!(lines(&merged, "[timing] repo=").is_empty(), "layouts are loaded, not built");
    let build = one_line(&merged, "[timing] build ");
    assert!(build.starts_with("[timing] build repos=2 "), "{build}");
    check_build_line(&build, false);
    let persist = one_line(&merged, "[timing] persist ");
    assert!(persist.starts_with("[timing] persist writer=merge "), "{persist}");
    assert!(persist.ends_with(&format!(" dir={}", out_dir.display())), "{persist}");
}
