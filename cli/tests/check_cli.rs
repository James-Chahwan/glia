//! LE.8 — `glia check`, driving the real binary: its exit code is the CI
//! contract. 1 when a declared rule is violated, 0 when every rule holds (or
//! none is declared), 2 when a rule cannot be evaluated.
//!
//! The `[check]` stderr line is the LE.8 fired_on marker, followed on a
//! violation by CC.3's `[check] tiers ..` line; asserting them here makes
//! their counts a tested contract, and relaying them lets
//! `cargo test -p glia-cli --test check_cli -- --nocapture 2>&1 | grep '^\[check\]'`
//! show it. A repo that declares a reflexion model (CC.5b) prints CC.5b's
//! `[reflexion]` line before them, relayed the same way (grep
//! `^\[reflexion\]`); CC.5c renders that model as the `## reflexion model`
//! section.

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

/// `glia <args>` with persistence off: `(exit code, stdout, [reflexion] and
/// [check] lines)`, the marker lines relayed to the test's stderr.
fn glia(args: &[&str]) -> (i32, String, Vec<String>) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let markers: Vec<String> = stderr
        .lines()
        .filter(|l| l.starts_with("[check]") || l.starts_with("[reflexion]"))
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

/// CC.5b's closed-layers fixture: web/app.py -> services/api/handlers.py ->
/// store/db.py, web/admin.py -> store/db.py (the layering violation), and
/// scripts/tool.py, which no component owns, -> store/db.py.
const REFLEXION_SOURCES: [(&str, &str); 6] = [
    ("store/pyproject.toml", "[project]\nname = \"store\"\n"),
    (
        "web/app.py",
        "from services.api.handlers import get_order\n\n\ndef show(o):\n    return get_order(o)\n",
    ),
    (
        "services/api/handlers.py",
        "from store.db import load\n\n\ndef get_order(o):\n    return load(o)\n",
    ),
    (
        "web/admin.py",
        "from store.db import load\n\n\ndef audit(o):\n    return load(o)\n",
    ),
    ("store/db.py", "def load(o):\n    return o\n"),
    ("scripts/tool.py", "from store.db import load\n"),
];

/// Components web (line 3), api (line 7), store (line 11); ui (strict) over
/// core over data, so web may use api only.
const MODEL: &str = "version = 1

[[component]]
name = \"web\"
paths = [\"web\"]

[[component]]
name = \"api\"
paths = [\"services/api\"]

[[component]]
name = \"store\"
paths = [\"store\"]

[[layer]]
name = \"ui\"
components = [\"web\"]
strict = true

[[layer]]
name = \"core\"
components = [\"api\"]

[[layer]]
name = \"data\"
components = [\"store\"]
";

/// The exception that lets web reach the store.
const ALLOW_WEB_STORE: &str = "
[[constraint]]
id = \"web-uses-store-cache\"
kind = \"allow\"
from = \"web\"
to = \"store\"
";

fn model_fixture(tag: &str, overlay: &str) -> Fixture {
    let files: Vec<(&str, &str)> = REFLEXION_SOURCES
        .iter()
        .copied()
        .chain(std::iter::once((".glia/overlay.toml", overlay)))
        .collect();
    Fixture::new(tag, &files)
}

#[test]
fn reflexion_section_and_exit_code() {
    // No allow: web -> store skips a layer below a strict one - a divergence,
    // which is a violation.
    let fx = model_fixture("reflexion-divergent", MODEL);
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 1, "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[reflexion] components=3 closed=true deps=3 convergences=2 divergences=1 absences=0 unmapped_files=1",
            "[check] rules=0 checked=0 violations=1 (forbid_edge=0 no_cycle=0) unchecked=0 errors=0",
            "[check] tiers fact=1 derived=0 heuristic=0",
        ]
    );
    assert!(
        !stdout.contains("_(no rules declared"),
        "a model with no rule is still checked: {stdout}"
    );
    for want in [
        "## reflexion model (closed)",
        "- web: web (layer ui, ",
        "- api: services/api (layer core, ",
        "| from | to | edges | status | tier | allowed by |",
        "| api | store | 2 | convergence | fact | layer:core>data |",
        "| web | api | 2 | convergence | fact | layer:ui>core |",
        "| web | store | 2 | divergence | fact | — |",
        "### absences",
        "### unmapped",
        "1 files, 1 nodes, 1 edges into components; e.g. scripts/tool.py",
        "## violations",
        "### reflexion:web->store (divergence, .glia/overlay.toml:3) - 2 violation(s)",
        "| 1 | IMPORTS | `web::admin` | `store::db` | web/admin.py:1 | graph:imports | fact |",
        "| 2 | CALLS | `web::admin::audit` | `store::db::load` | web/admin.py:5 | graph:calls | fact |",
    ] {
        assert!(stdout.contains(want), "missing {want:?}:\n{stdout}");
    }
    assert!(
        !stdout.contains("_cycle"),
        "a divergence is per edge, not a cycle: {stdout}"
    );
    let at = |s: &str| stdout.find(s).unwrap_or(usize::MAX);
    assert!(
        at("## reflexion model") < at("### reflexion:web->store"),
        "the model section comes before the violations: {stdout}"
    );

    // --json: the report carries the model, the same exit code.
    let (code, json, _) = glia(&["check", &fx.path(), "--json"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(json.trim()).expect("JSON report");
    assert_eq!(v["reflexion"]["closed"], true);
    assert_eq!(v["reflexion"]["divergences"], 1);
    assert_eq!(v["violations"][0]["rule_kind"], "divergence");

    // The allow restored: every dependency converges, exit 0.
    let fx = model_fixture("reflexion-convergent", &format!("{MODEL}{ALLOW_WEB_STORE}"));
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[reflexion] components=3 closed=true deps=3 convergences=3 divergences=0 absences=0 unmapped_files=1",
            "[check] rules=1 checked=1 violations=0 (forbid_edge=0 no_cycle=0) unchecked=0 errors=0",
        ]
    );
    assert!(
        stdout.contains("| web | store | 2 | convergence | fact | web-uses-store-cache |"),
        "{stdout}"
    );
    assert!(stdout.contains("_(no violations)_"), "{stdout}");
}

#[test]
fn reflexion_absence_lists_its_caveats() {
    // web -> api is realised; api -> web is allowed but no edge does it.
    let overlay = "version = 1

[[component]]
name = \"web\"
paths = [\"web\"]

[[component]]
name = \"api\"
paths = [\"services/api\"]

[[constraint]]
id = \"web-uses-api\"
kind = \"allow\"
from = \"web\"
to = \"api\"

[[constraint]]
id = \"api-uses-web\"
kind = \"allow\"
from = \"api\"
to = \"web\"
";
    let fx = Fixture::new(
        "reflexion-absence",
        &[
            ("web/app.py", WEB_APP),
            ("services/api/internal.py", INTERNAL),
            (".glia/overlay.toml", overlay),
        ],
    );
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 0, "an absence is never a violation: {stdout}");
    assert_eq!(
        markers.first().map(String::as_str),
        Some(
            "[reflexion] components=2 closed=true deps=1 convergences=1 divergences=0 absences=1 unmapped_files=0"
        ),
        "{markers:?}"
    );
    assert!(
        stdout.contains(
            "- api -> web: allowed by api-uses-web (.glia/overlay.toml:17), no edge found"
        ),
        "{stdout}"
    );
    let caveats = stdout.lines().filter(|l| l.starts_with("  ⚠ ")).count();
    assert!(
        (1..=5).contains(&caveats),
        "a blind extraction looks like an absence; at most 5 caveats per absence: {stdout}"
    );
    assert!(
        stdout.contains("_(none: every located file maps to a component)_"),
        "{stdout}"
    );
}
