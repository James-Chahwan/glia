//! LE.8 — `glia check`, driving the real binary: its exit code is the CI
//! contract. 1 when a declared rule is violated, 0 when every rule holds (or
//! none is declared), 2 when a rule cannot be evaluated.
//!
//! The `[check]` stderr line is the LE.8 fired_on marker, followed on a
//! violation by CC.3's `[check] tiers ..` line; asserting them here makes
//! their counts a tested contract, and relaying them lets
//! `cargo test -p glia-cli --test check_cli -- --nocapture 2>&1 | grep '^\[check\]'`
//! show it.

use std::path::PathBuf;
use std::process::Command;

const WEB_APP: &str =
    "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n";
const WEB_APP_CLEAN: &str = "def pay(o):\n    return o\n";
const INTERNAL: &str = "def charge(o):\n    return o\n";
const API_A: &str = "from services.api.b import g\n\n\ndef f():\n    return g()\n";
const API_B: &str = "from services.api.a import f\n\n\ndef g():\n    return 1\n";

const RULES: &str = "version = 1

[[constraint]]
id = \"web-no-api-internals\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
categories = [\"IMPORTS\", \"CALLS\"]

[[constraint]]
id = \"api-acyclic\"
kind = \"no_cycle\"
scope = \"services/api\"

[[constraint]]
id = \"prose\"
kind = \"invariant\"
scope = \"services/api\"
text = \"charges are idempotent\"
";

/// `to` names a path no file sits under: the rule cannot be evaluated.
const GHOST_RULE: &str = "version = 1

[[constraint]]
id = \"web-no-ghost\"
kind = \"forbid_edge\"
from = \"web\"
to = \"ghost\"
";

/// A per-test directory under the system temp dir, removed on drop (the cli
/// crate has no `tempfile` dev-dependency). Two manifest projects so LF.4a
/// anchors every rule on a PROJECT.
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str, files: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("glia-check-cli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let manifests = [
            ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
            ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
        ];
        for (rel, text) in manifests.iter().chain(files) {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
            std::fs::write(p, text).expect("write fixture file");
        }
        Fixture(root)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `glia <args>` with persistence off: `(exit code, stdout, [check] lines)`,
/// the marker lines relayed to the test's stderr.
fn glia(args: &[&str]) -> (i32, String, Vec<String>) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let markers: Vec<String> = stderr
        .lines()
        .filter(|l| l.starts_with("[check]"))
        .map(str::to_string)
        .collect();
    for m in &markers {
        eprintln!("{m}");
    }
    let code = out.status.code().unwrap_or(-1);
    (
        code,
        String::from_utf8_lossy(&out.stdout).into_owned(),
        markers,
    )
}

#[test]
fn violations_exit_1_with_located_tables() {
    let fx = Fixture::new(
        "violating",
        &[
            ("web/app.py", WEB_APP),
            ("services/api/internal.py", INTERNAL),
            ("services/api/a.py", API_A),
            ("services/api/b.py", API_B),
            (".glia/overlay.toml", RULES),
        ],
    );
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 1, "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[check] rules=3 checked=2 violations=2 (forbid_edge=1 no_cycle=1) unchecked=1 errors=0",
            "[check] tiers fact=1 derived=1 heuristic=0",
        ]
    );
    assert!(
        stdout.contains(
            "### web-no-api-internals (forbid_edge, .glia/overlay.toml:3) - 2 violation(s)"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("| # | category | from | to | at | emitter | tier |"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| 1 | IMPORTS | `web::app` | `services::api::internal` | web/app.py:1 | graph:imports | fact |"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| 2 | CALLS | `web::app::pay` | `services::api::internal::charge` | web/app.py:5 | graph:calls | fact |"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("### api-acyclic (no_cycle, .glia/overlay.toml:10) - 1 violation(s)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "| 1 | IMPORTS | `services::api::a` | `services::api::b` | services/api/a.py:1 |"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("- `prose`"),
        "the invariant is listed: {stdout}"
    );

    // --json: the same report, the same exit code.
    let (code, stdout, _) = glia(&["check", &fx.path(), "--json"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).expect("JSON report");
    assert_eq!(v["rules"], 3);
    assert_eq!(v["checked"], 2);
    assert_eq!(v["unchecked"], serde_json::json!(["prose"]));
    assert_eq!(v["violations"][0]["rule_id"], "api-acyclic");
    assert_eq!(v["violations"][1]["rule_id"], "web-no-api-internals");
    assert_eq!(v["violations"][1]["count"], 2);
    assert_eq!(v["violations"][1]["evidence"][1]["line"], 5);
    assert_eq!(v["violations"][1]["evidence"][1]["tier"], "fact");
    assert_eq!(v["violations"][0]["tier"], "derived");
}

#[test]
fn clean_repo_exits_0() {
    let fx = Fixture::new(
        "clean",
        &[
            ("web/app.py", WEB_APP_CLEAN),
            ("services/api/internal.py", INTERNAL),
            (".glia/overlay.toml", RULES),
        ],
    );
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[check] rules=3 checked=2 violations=0 (forbid_edge=0 no_cycle=0) unchecked=1 errors=0"
        ]
    );
    assert!(stdout.contains("_(no violations)_"), "{stdout}");
}

#[test]
fn no_rules_exits_0_and_says_how_to_declare_one() {
    let fx = Fixture::new("norules", &[("web/app.py", WEB_APP)]);
    let (code, stdout, _) = glia(&["check", &fx.path()]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("_(no rules declared - add [[constraint]] stanzas to .glia/overlay.toml)_"),
        "{stdout}"
    );
}

#[test]
fn rule_error_exits_2() {
    let fx = Fixture::new(
        "ghost",
        &[
            ("web/app.py", WEB_APP),
            ("services/api/internal.py", INTERNAL),
            (".glia/overlay.toml", GHOST_RULE),
        ],
    );
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[check] rules=1 checked=0 violations=0 (forbid_edge=0 no_cycle=0) unchecked=0 errors=1"
        ]
    );
    assert!(stdout.contains("## rule errors"), "{stdout}");
    assert!(
        stdout.contains("- `web-no-ghost`: to scope `ghost`"),
        "{stdout}"
    );
}

#[test]
fn missing_repo_exits_2() {
    let (code, _, markers) = glia(&["check", "/nonexistent/glia-check-cli-no-such-repo"]);
    assert_eq!(code, 2);
    assert!(markers.is_empty(), "no check ran: {markers:?}");
}
