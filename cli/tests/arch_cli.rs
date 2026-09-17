//! A9.6 — `glia arch` golden checks, driving the real binary.
//!
//! The `[arch] …` stderr line is the A9 fired_on marker; asserting it here is
//! what makes it a tested contract rather than a convention.

use std::path::PathBuf;
use std::process::{Command, Output};

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

fn glia_arch(flag: &str) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["arch", &fixture("arch_monorepo"), flag])
        .output()
        .expect("glia runs");
    assert!(
        out.status.success(),
        "glia arch {flag} exited {:?}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // Relay the child's marker so `-- --nocapture | grep '\[arch\] '` sees it.
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[arch] ") {
            eprintln!("{line}");
        }
    }
    out
}

#[test]
fn arch_json_lists_services_and_links() {
    let out = glia_arch("--json");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[arch] 2 services, 2 links (keying=top_level_dir "),
        "fired_on marker missing or wrong:\n{stderr}"
    );

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(v["keying"], "top_level_dir");
    let ids: Vec<&str> = v["services"]
        .as_array()
        .expect("services array")
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    assert_eq!(ids, ["api", "web"]);
    let links = v["links"].as_array().expect("links array");
    let mut rows: Vec<String> = links
        .iter()
        .map(|l| {
            format!(
                "{} -> {} {} {}",
                l["from"].as_str().unwrap_or("?"),
                l["to"].as_str().unwrap_or("?"),
                l["mechanism"].as_str().unwrap_or("?"),
                l["channel"].as_str().unwrap_or("?"),
            )
        })
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        [
            "web -> api HTTP_CALLS GET /users",
            "web -> api HTTP_CALLS POST /users",
        ]
    );
}

#[test]
fn arch_mermaid_emits_a_link() {
    let out = glia_arch("--mermaid");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("```mermaid"), "{stdout}");
    assert!(stdout.contains("graph LR"), "{stdout}");
    // Nodes are positional (`svc{i}` over the sorted services), names in labels.
    assert!(stdout.contains(r#"svc0["api"]"#), "{stdout}");
    assert!(stdout.contains(r#"svc1["web"]"#), "{stdout}");
    // One arrow per (from, to, mechanism): web → api, both channels collapsed.
    let arrows: Vec<&str> = stdout.lines().filter(|l| l.contains(" -->|")).collect();
    assert_eq!(arrows.len(), 1, "{stdout}");
    assert!(
        arrows[0].trim_start().starts_with("svc1 -->|\"HTTP_CALLS ×2")
            && arrows[0].trim_end().ends_with("| svc0"),
        "{}",
        arrows[0]
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("[arch] 2 services,"));
}
