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
        stderr.contains("[arch] 2 services, 2 links (keying=project_roots "),
        "fired_on marker missing or wrong:\n{stderr}"
    );

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    // `web/` and `api/` each carry a manifest, so they are A8.5 project roots.
    assert_eq!(v["keying"], "project_roots");
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

// ----------------------------------------------------------------------------
// DD.14 — identity classes in `glia arch`
// ----------------------------------------------------------------------------

/// Three repos declaring react (one package_name class, stored as a two-edge
/// star), each with its own code, under cargo's per-test scratch dir.
struct ThreeRepos {
    root: PathBuf,
}

impl ThreeRepos {
    fn new(tag: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("dd14-arch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for repo in ["alpha", "beta", "gamma"] {
            let dir = root.join(repo);
            std::fs::create_dir_all(dir.join("src")).expect("mkdir");
            std::fs::write(
                dir.join("package.json"),
                format!(
                    "{{\"name\": \"{repo}\", \"version\": \"1.0.0\", \"dependencies\": {{\"react\": \"^18.2.0\"}}}}\n"
                ),
            )
            .expect("write package.json");
            std::fs::write(
                dir.join("src/index.js"),
                format!("export function {repo}Main() {{\n  return \"{repo}\";\n}}\n"),
            )
            .expect("write index.js");
        }
        Self { root }
    }

    /// `glia arch alpha --with beta --with gamma <flags>`: (SAME_AS link rows
    /// `from -> to`, stderr).
    fn arch(&self, flags: &[&str]) -> (Vec<String>, String) {
        let path = |r: &str| self.root.join(r).to_string_lossy().into_owned();
        let (beta, gamma) = (path("beta"), path("gamma"));
        let mut args = vec!["arch".to_string(), path("alpha")];
        for w in [&beta, &gamma] {
            args.push("--with".to_string());
            args.push(w.clone());
        }
        args.extend(flags.iter().map(|f| f.to_string()));
        let out = Command::new(env!("CARGO_BIN_EXE_glia"))
            .args(&args)
            .env("GLIA_NO_PERSIST", "1")
            .output()
            .expect("glia runs");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "glia {args:?} exited {:?}\n{stderr}", out.status);
        for line in stderr.lines() {
            if line.starts_with("[arch] ") || line.starts_with("[identity] collapse surface=arch") {
                eprintln!("{line}");
            }
        }
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
        let rows = v["links"]
            .as_array()
            .expect("links array")
            .iter()
            .filter(|l| l["mechanism"] == "SAME_AS")
            .map(|l| {
                format!(
                    "{} -> {} {} x{} {}",
                    l["from"].as_str().unwrap_or("?"),
                    l["to"].as_str().unwrap_or("?"),
                    l["channel"].as_str().unwrap_or("?"),
                    l["count"],
                    l["confidence"].as_str().unwrap_or("?"),
                )
            })
            .collect();
        (rows, stderr)
    }
}

impl Drop for ThreeRepos {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn arch_same_as_class_links_every_service_pair() {
    let repos = ThreeRepos::new("collapse");
    let (rows, stderr) = repos.arch(&["--include-shared", "--json"]);
    assert_eq!(
        rows,
        [
            "alpha -> beta react x1 strong",
            "alpha -> gamma react x1 strong",
            "beta -> gamma react x1 strong",
        ]
    );
    assert!(
        stderr.contains("[identity] collapse surface=arch classes=1 walk_classes=0 folded=0 rows=3"),
        "fired_on marker missing or wrong:\n{stderr}"
    );
    assert!(stderr.contains("[arch] 3 services, 3 links (keying=per_repo "), "{stderr}");

    // --no-collapse: the stored star, two links into the center's service.
    let (raw, raw_err) = repos.arch(&["--include-shared", "--json", "--no-collapse"]);
    assert_eq!(raw.len(), 2, "{raw:?}");
    assert!(raw.iter().all(|r| r.ends_with(" react x1 strong")), "{raw:?}");
    assert!(!raw_err.contains("[identity] collapse surface=arch"), "off prints no collapse line");

    // Without --include-shared no SAME_AS link is shown, collapsed or not.
    let (hidden, _) = repos.arch(&["--json"]);
    assert!(hidden.is_empty(), "{hidden:?}");
    let (hidden_raw, _) = repos.arch(&["--json", "--no-collapse"]);
    assert!(hidden_raw.is_empty(), "{hidden_raw:?}");
}

#[test]
fn arch_no_collapse_is_identical_on_a_class_free_stack() {
    let collapsed = glia_arch("--json");
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["arch", &fixture("arch_monorepo"), "--json", "--no-collapse"])
        .output()
        .expect("glia runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout, collapsed.stdout, "no SAME_AS edge: one answer");
}
