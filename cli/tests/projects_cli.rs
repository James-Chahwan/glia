//! A8.6 — `glia projects` and `--scope <label>`, driving the real binary over
//! the committed A8.5 fixture (four manifest roots under one RepoId).
//!
//! The `[scope] resolved label` stderr line is the A8.6 fired_on marker;
//! asserting it here makes it a tested contract rather than a convention.

use std::process::{Command, Output};

fn fixture() -> String {
    format!(
        "{}/../bench/substrate-gap/fixtures/walk-project-roots",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn glia(args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .output()
        .expect("glia runs");
    assert!(
        out.status.success(),
        "glia {args:?} exited {:?}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // Relay the markers so `-- --nocapture | grep '^\[scope\] '` sees them.
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[scope] ") || line.starts_with("[projects] ") {
            eprintln!("{line}");
        }
    }
    out
}

#[test]
fn projects_json_lists_the_four_roots_sorted_by_path() {
    let out = glia(&["projects", &fixture(), "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let rows: Vec<(String, String)> = v
        .as_array()
        .expect("a JSON array")
        .iter()
        .map(|p| {
            (
                p["path"].as_str().unwrap_or_default().to_string(),
                p["label"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let want = [
        (".", "shop-monorepo"),
        ("apps/web", "@shop/web"),
        ("libs/core", "shop-core"),
        ("services/api", "github.com/shop/api"),
    ];
    assert_eq!(
        rows,
        want.map(|(p, l)| (p.to_string(), l.to_string())),
        "{v}"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("[projects] surface=cli roots=4"));
}

#[test]
fn scope_accepts_a_project_label() {
    let out = glia(&["blast-radius", &fixture(), "webEntry", "--scope", "@shop/web"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[scope] resolved label '@shop/web' -> apps/web"),
        "the label must resolve to its path:\n{stderr}"
    );
    assert!(
        stderr.contains("[scope] blast_radius scope=apps/web:"),
        "the filter must run on the RESOLVED path:\n{stderr}"
    );

    // A literal path never prints the label marker.
    let out = glia(&["blast-radius", &fixture(), "webEntry", "--scope", "apps/web"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("resolved label"), "{stderr}");
}
