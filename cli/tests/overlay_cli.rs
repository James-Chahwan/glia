//! LF.2b — the global `--no-overlay` flag, driving the real binary over the
//! committed `xcut-overlay-edges` fixture (a web repo whose
//! `.glia/overlay.toml` declares a CALLS edge into the api repo).
//!
//! The `[overlay] edges ...` and `[overlay] disabled (--no-overlay) ...`
//! stderr lines are the fired_on markers; asserting them here makes them a
//! tested contract. The persist guard (a no-overlay graph never goes to the
//! default layout dir) is `glia build --no-overlay` refusing to run without
//! `--out`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(sub: &str) -> String {
    format!("{}/../bench/substrate-gap/fixtures/xcut-overlay-edges/{sub}", env!("CARGO_MANIFEST_DIR"))
}

fn glia(args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia")).args(args).output().expect("glia runs");
    // Relay the markers so `-- --nocapture | grep '^\[overlay\] '` sees them.
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[overlay] ") {
            eprintln!("{line}");
        }
    }
    out
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A private copy of the fixture's web repo, so a build may write into it.
fn web_copy(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("glia-lf2b-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let web = root.join("web");
    std::fs::create_dir_all(web.join("src")).unwrap();
    std::fs::create_dir_all(web.join(".glia")).unwrap();
    for rel in ["src/report.ts", ".glia/overlay.toml"] {
        std::fs::copy(Path::new(&fixture("web")).join(rel), web.join(rel)).unwrap();
    }
    web
}

#[test]
fn merge_applies_the_overlay_by_default() {
    let out = glia(&["merge", &fixture("web"), &fixture("api")]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "exit {:?}\n{stderr}", out.status);
    assert!(
        stderr.contains("declared=2 applied=1 redundant=0 orphaned=0 rejected=1 (llm=1 human=0)"),
        "{stderr}"
    );
    assert!(stdout.contains("- cross-edges: 1"), "{stdout}");
}

#[test]
fn no_overlay_merge_skips_the_overlay() {
    // The flag is global: before the subcommand and after it mean the same.
    for args in [
        vec!["--no-overlay", "merge", &fixture("web"), &fixture("api")],
        vec!["merge", &fixture("web"), &fixture("api"), "--no-overlay"],
    ]
    .map(|a| a.into_iter().map(String::from).collect::<Vec<_>>())
    {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = glia(&args);
        let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
        assert!(out.status.success(), "{args:?}: exit {:?}\n{stderr}", out.status);
        assert!(stderr.contains("[overlay] disabled (--no-overlay) repo="), "{args:?}: {stderr}");
        assert!(!stderr.contains("[overlay] edges "), "{args:?}: {stderr}");
        assert!(stdout.contains("- cross-edges: 0"), "{args:?}: {stdout}");
    }
}

#[test]
fn no_overlay_build_needs_out() {
    let web = web_copy("build");
    let web_s = web.to_string_lossy().into_owned();
    let out = glia(&["build", &web_s, "--no-overlay"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("error: --no-overlay needs --out"), "{}", text(&out.stderr));
    assert!(!web.join(".glia/graph").exists(), "the refusal wrote nothing");

    // With --out the extraction-only layout goes there, never to the default dir.
    let dest = web.parent().unwrap().join("bare-layout");
    let dest_s = dest.to_string_lossy().into_owned();
    let out = glia(&["--no-overlay", "build", &web_s, "--out", &dest_s, "--no-incremental"]);
    assert!(out.status.success(), "exit {:?}\n{}", out.status, text(&out.stderr));
    assert!(dest.join("manifest.json").is_file());
    assert!(!web.join(".glia/graph/manifest.json").exists());
    let _ = std::fs::remove_dir_all(web.parent().unwrap());
}

#[test]
fn no_overlay_layout_merge_is_refused() {
    let web = web_copy("layout");
    let out = glia(&["--no-overlay", "merge", "--gmap", &web.join(".glia/graph").to_string_lossy()]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("--no-overlay applies to a source merge only"), "{}", text(&out.stderr));
    let _ = std::fs::remove_dir_all(web.parent().unwrap());
}
