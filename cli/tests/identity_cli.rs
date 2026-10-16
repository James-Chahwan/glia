//! DD.15 — `glia identity`, driving the real binary over temp copies of
//! DD.5's and DD.6's substrate-gap identity fixtures:
//!
//! - `identity-npm-package`: `web` and `admin` both depend on `react` and
//!   `lodash`, two FACT `package_name` classes (collapsed in answers);
//! - `identity-vendored-subtree`: `shop` copies `money` under
//!   `third_party/money/`, `format.ts` byte-identical (`body_hash`) and
//!   `parse.ts` diverged (`path_suffix`, two variants): HEURISTIC same-as
//!   candidates, never collapsed.
//!
//! Baseline before DD.15 (HEAD c675fe47): `glia identity` exited 2 with
//! `error: unrecognized subcommand 'identity'`.
//!
//! The engine's `[identity] report classes=<c> fact=<f> derived=<d>
//! heuristic=<h> diverged=<x>` stderr line is the fired_on marker; every
//! answering run asserts it and relays it, so
//! `cargo test -p glia-cli --test identity_cli -- --nocapture | grep '^\[identity\] report'`
//! sees it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const MARKER: &str = "[identity] report classes=";

/// A temp copy of one substrate-gap fixture, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(fixture: &str, tag: &str) -> Self {
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../bench/substrate-gap/fixtures")
            .join(fixture);
        let dst = std::env::temp_dir().join(format!("glia-dd15-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dst);
        copy_tree(&src, &dst);
        Root(dst)
    }

    fn repo(&self, dir: &str) -> String {
        self.0.join(dir).to_string_lossy().into_owned()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every file under `src` copied to the same place under `dst`, `key.json`
/// left out (it is the grader's, not a source).
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let to = dst.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &to);
        } else if entry.file_name() != "key.json" {
            std::fs::copy(&path, &to).expect("copy fixture file");
        }
    }
}

/// `glia identity <args>`, checking the exit code; an answering run (exit 0
/// or 1) prints the marker exactly once, and it is relayed.
fn glia(args: &[&str], want: i32) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .arg("identity")
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(want),
        "glia identity {args:?}\nstdout:\n{}\nstderr:\n{stderr}",
        stdout(&out)
    );
    let markers: Vec<&str> = stderr.lines().filter(|l| l.starts_with(MARKER)).collect();
    if want != 2 {
        assert_eq!(markers.len(), 1, "one report marker:\n{stderr}");
    }
    for line in markers {
        eprintln!("{line}");
    }
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).expect("--json prints one JSON document")
}

fn marker(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .find(|l| l.starts_with(MARKER))
        .unwrap_or_default()
        .to_string()
}

/// `(key, rules, tier, diverged, variants)` of each listed class.
fn classes(report: &Value) -> Vec<(String, Vec<String>, String, bool, u64)> {
    report["classes"]
        .as_array()
        .expect("classes")
        .iter()
        .map(|c| {
            (
                c["key"].as_str().unwrap_or_default().to_string(),
                c["rules"]
                    .as_array()
                    .expect("rules")
                    .iter()
                    .filter_map(|r| r.as_str().map(str::to_string))
                    .collect(),
                c["tier"].as_str().unwrap_or_default().to_string(),
                c["diverged"].as_bool().unwrap_or_default(),
                c["variants"].as_u64().unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn npm_package_report() {
    let root = Root::new("identity-npm-package", "npm");
    let (web, admin) = (root.repo("web"), root.repo("admin"));
    let out = glia(&[&web, "--with", &admin, "--json"], 0);
    let r = json(&out);
    assert_eq!(
        marker(&out),
        "[identity] report classes=2 fact=2 derived=0 heuristic=0 diverged=0"
    );
    let counts = &r["counts"];
    assert_eq!(
        (
            counts["classes"].as_u64(),
            counts["members"].as_u64(),
            counts["fact"].as_u64(),
            counts["heuristic"].as_u64(),
        ),
        (Some(2), Some(4), Some(2), Some(0)),
        "{counts}"
    );
    assert_eq!(counts["by_rule"][0], serde_json::json!(["package_name", 2]));
    assert_eq!(
        classes(&r)
            .into_iter()
            .map(|(key, rules, tier, diverged, _)| (key, rules, tier, diverged))
            .collect::<Vec<_>>(),
        ["package:npm:lodash", "package:npm:react"]
            .map(|k| (
                k.to_string(),
                vec!["package_name".to_string()],
                "fact".to_string(),
                false
            ))
            .to_vec()
    );
    for c in r["classes"].as_array().expect("classes") {
        assert_eq!(c["collapses"], true, "{c}");
        let repos: Vec<&str> = c["members"]
            .as_array()
            .expect("members")
            .iter()
            .filter_map(|m| m["repo"].as_str())
            .collect();
        assert_eq!(repos, ["web", "admin"], "display member first: {c}");
        assert!(
            c["members"]
                .as_array()
                .expect("members")
                .iter()
                .all(|m| m["kind"] == "PACKAGE_DEP" && m["qname"] == c["key"]),
            "{c}"
        );
    }
    assert!(r["absence"].is_null());
    assert_eq!(r["omitted"], 0);

    let text = stdout(&glia(&[&web, "--with", &admin], 0));
    assert!(
        text.starts_with(&format!("# glia identity {web} (+1 repo)\n")),
        "{text}"
    );
    assert!(
        text.contains("\nclasses=2 members=4 fact=2 derived=0 heuristic=0 diverged=0\n"),
        "{text}"
    );
    assert!(text.contains("\nrules: package_name=2\n"), "{text}");
    assert!(
        text.contains(
            "## collapsed in answers (fact / derived)\n\n\
             - package:npm:lodash [package_name] fact\n  web — PACKAGE_DEP package:npm:lodash\n  admin — PACKAGE_DEP package:npm:lodash\n"
        ),
        "{text}"
    );
    assert!(!text.contains("## same-as candidates"), "{text}");
}

#[test]
fn vendored_candidates() {
    let root = Root::new("identity-vendored-subtree", "vendored");
    let (money, shop) = (root.repo("money"), root.repo("shop"));
    let r = json(&glia(
        &[&money, "--with", &shop, "--tier", "heuristic", "--json"],
        0,
    ));
    let got = classes(&r);
    let class = |key: &str| {
        got.iter()
            .find(|c| c.0 == key)
            .unwrap_or_else(|| panic!("no class {key}: {got:?}"))
            .clone()
    };
    assert_eq!(
        class("src::format::formatCents"),
        (
            "src::format::formatCents".to_string(),
            vec!["body_hash".to_string()],
            "heuristic".to_string(),
            false,
            1
        )
    );
    assert_eq!(
        class("src::parse::parseCents"),
        (
            "src::parse::parseCents".to_string(),
            vec!["path_suffix".to_string()],
            "heuristic".to_string(),
            true,
            2
        )
    );
    assert!(got.iter().all(|c| c.2 == "heuristic"), "{got:?}");
    assert!(
        r["classes"]
            .as_array()
            .expect("classes")
            .iter()
            .all(|c| c["collapses"] == false && c["walks"] == false),
        "candidates never collapse or walk"
    );
    let parse = r["classes"]
        .as_array()
        .expect("classes")
        .iter()
        .find(|c| c["key"] == "src::parse::parseCents")
        .expect("parseCents class");
    let members: Vec<(&str, &str, i64, u64)> = parse["members"]
        .as_array()
        .expect("members")
        .iter()
        .map(|m| {
            (
                m["repo"].as_str().unwrap_or_default(),
                m["file"].as_str().unwrap_or_default(),
                m["line"].as_i64().unwrap_or_default(),
                m["variant"].as_u64().unwrap_or(99),
            )
        })
        .collect();
    assert_eq!(
        members,
        [
            ("money", "src/parse.ts", 1, 0),
            ("shop", "third_party/money/src/parse.ts", 1, 1)
        ]
    );

    let text = stdout(&glia(&[&money, "--with", &shop, "--tier", "heuristic"], 0));
    assert!(
        text.contains("## same-as candidates (heuristic, never collapsed)\n"),
        "{text}"
    );
    assert!(!text.contains("## collapsed in answers"), "{text}");
    assert!(
        text.contains(
            "- src::parse::parseCents [path_suffix] heuristic diverged, 2 variants\n  money src/parse.ts:1 FUNCTION src::parse::parseCents (variant 0)\n  shop third_party/money/src/parse.ts:1 FUNCTION third_party::money::src::parse::parseCents (variant 1)\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("- src::format::formatCents [body_hash] heuristic\n  money src/format.ts:1 FUNCTION src::format::formatCents\n"),
        "no variant shown for one content: {text}"
    );

    let diverged = json(&glia(
        &[&money, "--with", &shop, "--diverged-only", "--json"],
        0,
    ));
    let kept = classes(&diverged);
    assert!(!kept.is_empty() && kept.iter().all(|c| c.3), "{kept:?}");
    assert!(
        kept.iter().any(|c| c.0 == "src::parse::parseCents"),
        "{kept:?}"
    );
    assert!(
        kept.iter().all(|c| c.0 != "src::format::formatCents"),
        "{kept:?}"
    );
    assert_eq!(
        diverged["counts"]["diverged"],
        diverged["counts"]["classes"]
    );

    let rule = json(&glia(
        &[&money, "--with", &shop, "--rule", "body_hash", "--json"],
        0,
    ));
    assert!(
        classes(&rule).iter().all(|c| c.1 == ["body_hash"]),
        "{rule}"
    );
}

#[test]
fn limit_cuts_listing_not_counts() {
    let root = Root::new("identity-npm-package", "limit");
    let (web, admin) = (root.repo("web"), root.repo("admin"));
    let r = json(&glia(
        &[&web, "--with", &admin, "--limit", "1", "--json"],
        0,
    ));
    assert_eq!(r["classes"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        (r["omitted"].as_u64(), r["counts"]["classes"].as_u64()),
        (Some(1), Some(2))
    );
    let text = stdout(&glia(&[&web, "--with", &admin, "--limit", "1"], 0));
    assert!(text.ends_with("… 1 more (--limit 0 for all)\n"), "{text}");
    let all = json(&glia(
        &[&web, "--with", &admin, "--limit", "0", "--json"],
        0,
    ));
    assert_eq!(
        (
            all["classes"].as_array().map(Vec::len),
            all["omitted"].as_u64()
        ),
        (Some(2), Some(0))
    );
}

#[test]
fn bad_rule() {
    let root = Root::new("identity-npm-package", "bad");
    let web = root.repo("web");
    let out = glia(&[&web, "--rule", "nope"], 2);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error: invalid value 'nope'"), "{stderr}");
    for rule in glia_code_domain::same_as::RULES {
        assert!(stderr.contains(rule), "names {rule}: {stderr}");
    }
    assert!(stdout(&out).is_empty());
    let tier = glia(&[&web, "--tier", "maybe"], 2);
    let stderr = String::from_utf8_lossy(&tier.stderr);
    assert!(
        stderr.contains("fact") && stderr.contains("derived") && stderr.contains("heuristic"),
        "{stderr}"
    );
}

#[test]
fn empty() {
    let root = Root::new("identity-npm-package", "empty");
    let web = root.repo("web");
    let out = glia(&[&web], 1);
    assert_eq!(
        marker(&out),
        "[identity] report classes=0 fact=0 derived=0 heuristic=0 diverged=0"
    );
    let text = stdout(&out);
    assert!(text.contains("_(no identity classes)_"), "{text}");
    assert!(text.contains("> FACT: no SAME_AS edge"), "{text}");
    let r = json(&glia(&[&web, "--json"], 1));
    assert_eq!(r["classes"], serde_json::json!([]));
    assert_eq!(r["absence"]["reason"], "no_match", "{r}");

    // Filters that remove every class are an absence too, naming them.
    let admin = root.repo("admin");
    let none = json(&glia(
        &[&web, "--with", &admin, "--tier", "derived", "--json"],
        1,
    ));
    assert!(
        none["absence"]["note"]
            .as_str()
            .is_some_and(|n| n.contains("filters removed 2 classes")),
        "{none}"
    );
}
