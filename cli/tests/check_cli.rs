//! LE.8 — `glia check`, driving the real binary: its exit code is the CI
//! contract. 1 when a violation is BLOCKING (DB.3d: its tier reaches its
//! rule's `min_tier` and is not heuristic), 0 when nothing blocks (every rule
//! holds, none is declared, or every violation is advisory), 2 when a rule
//! cannot be evaluated.
//!
//! DB.3d renders DB.3a's rule-invariant rows (`_violation <k>: ..`, with the
//! subject, its location and the witness edges; the rule's Q5 caveats print
//! once under its heading, DT.8a) and DB.3b's transitive forbid_edge hits
//! (`_path <k> into ..`, the witness in path order), marks an advisory
//! violation on its tier line, and prints the summary `- violations: <V>
//! (blocking <B>, advisory <A>)`.
//!
//! DT.7b lists the declared rules that check nothing (a stanza the build
//! anchored nowhere, rejected or does not hold; a rule whose scope no located
//! node sits in) under `## Rules that check nothing`, never changing the exit
//! code; `--strict` makes each a rule error, exit 2. The `[check] rules=`
//! line then ends ` unanchored=<n>`, and `[check] declared not checked:
//! orphaned=<o> rejected=<r> absent=<a> empty_scope=<e> blocking=<b>`
//! follows it.
//!
//! The `[check]` stderr line is the LE.8 fired_on marker, followed on a
//! violation by CC.3's `[check] tiers ..` line and DB.3a's `[check] gate
//! blocking=<B> advisory=<A>` line, then, when a rule printed its caveat
//! block (DT.8a: once under the rule heading, not under each row), the
//! `[check] caveats rules=<r> lines=<l> folded=<f>` line; asserting them
//! here makes their counts a tested contract, and relaying them lets
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

/// The root PROJECT an unscoped invariant anchors on (LF.4a).
const ROOT_MANIFEST: (&str, &str) = ("pyproject.toml", "[project]\nname = \"root\"\n");

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
            "[check] gate blocking=2 advisory=0",
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

/// DT.7b: a rule whose scope no located node sits in checks nothing. It is
/// listed under `## Rules that check nothing` with a fix and does not fail
/// CI (HEAD exited 2 on it); `--strict` makes it the rule error HEAD printed,
/// its fix under it, exit 2.
#[test]
fn empty_scope_checks_nothing_unless_strict() {
    let fx = Fixture::new(
        "ghost",
        &[
            ("web/app.py", WEB_APP),
            ("services/api/internal.py", INTERNAL),
            (".glia/overlay.toml", GHOST_RULE),
        ],
    );
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[check] rules=1 checked=0 violations=0 (forbid_edge=0 no_cycle=0) unchecked=0 errors=0 unanchored=1",
            "[check] declared not checked: orphaned=0 rejected=0 absent=0 empty_scope=1 blocking=0",
        ]
    );
    assert!(
        stdout.contains("- rules: 1 (checked 0, unchecked 0, errors 0, checking nothing 1)"),
        "{stdout}"
    );
    assert!(!stdout.contains("## rule errors"), "{stdout}");
    let section = &stdout[stdout
        .find("## Rules that check nothing")
        .expect("the rules that check nothing")..];
    assert!(
        section.contains(
            "- `web-no-ghost` (constraint, .glia/overlay.toml:3): to scope `ghost` matches no located node (no file sits under it and no PROJECT is at it)\n  fix: check the spelling of `to`"
        ),
        "{section}"
    );

    let (code, stdout, markers) = glia(&["check", &fx.path(), "--strict"]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[check] rules=1 checked=0 violations=0 (forbid_edge=0 no_cycle=0) unchecked=0 errors=1 unanchored=1",
            "[check] declared not checked: orphaned=0 rejected=0 absent=0 empty_scope=1 blocking=1",
        ]
    );
    assert!(stdout.contains("## rule errors"), "{stdout}");
    assert!(
        stdout.contains("- `web-no-ghost`: to scope `ghost` matches no located node (no file sits under it and no PROJECT is at it)\n  fix: "),
        "{stdout}"
    );
    assert!(!stdout.contains("## Rules that check nothing"), "{stdout}");
}

/// Rule invariants that do not analyse (DB.3a): errors, never checked, each
/// diagnostic printed verbatim on its own line, in source order; exit 2 with
/// or without `--strict`.
#[test]
fn rule_error_exits_2() {
    let rotten = r#"version = 2

[[constraint]]
id = "rotten"
kind = "invariant"
text = "a rule naming an unknown kind"
rule = "violation(r) :- node(r, \"ROUTEE\")."

[[constraint]]
id = "rotten2"
kind = "invariant"
text = "a rule naming two unknown names"
rule = "violation(r) :- node(r, \"ROUTEE\"), edge(r, _, \"CALLZ\")."
"#;
    let fx = Fixture::new(
        "rotten",
        &[
            ROOT_MANIFEST,
            ("web/app.py", WEB_APP),
            ("services/api/internal.py", INTERNAL),
            (".glia/overlay.toml", rotten),
        ],
    );
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[check] rules=2 checked=0 violations=0 (forbid_edge=0 no_cycle=0) unchecked=0 errors=2"
        ]
    );
    let errors = &stdout[stdout
        .find("## rule errors (not evaluated)")
        .expect("the rule errors section")..];
    let line = errors
        .lines()
        .find(|l| l.starts_with("- `rotten`: "))
        .unwrap_or_else(|| panic!("rotten is listed: {stdout}"));
    assert!(
        line.starts_with("- `rotten`: rotten (.glia/overlay.toml:3):1:")
            && line.contains("unknown node kind `ROUTEE`"),
        "{line}"
    );
    let two: Vec<&str> = errors
        .lines()
        .skip_while(|l| !l.starts_with("- `rotten2`: "))
        .take(2)
        .collect();
    assert!(
        two.len() == 2
            && two[0].starts_with("- `rotten2`: rotten2 (.glia/overlay.toml:9):1:25: error: unknown node kind `ROUTEE`")
            && two[1].starts_with("  rotten2 (.glia/overlay.toml:9):1:47: error: unknown edge category `CALLZ`"),
        "one diagnostic per line, the second under the bullet: {errors}"
    );
    assert!(
        stdout.contains("- violations: 0 (blocking 0, advisory 0)"),
        "{stdout}"
    );
    let (strict_code, strict_out, _) = glia(&["check", &fx.path(), "--strict"]);
    assert_eq!((strict_code, strict_out), (code, stdout));
}

/// DT.7a's substrate-gap fixture, copied: its `stale-anchor` stanza names a
/// qname no node carries.
const SUBDIR_FIXTURE: &str = "../bench/substrate-gap/fixtures/overlay-subdir-constraint";

/// `from` copied into `to`, recursively.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

/// [`SUBDIR_FIXTURE`] under a per-test temp dir (removed on drop), its
/// overlay replaced by `overlay` when given.
fn subdir_fixture(tag: &str, overlay: Option<&str>) -> Fixture {
    let root = std::env::temp_dir().join(format!("glia-check-cli-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(SUBDIR_FIXTURE);
    copy_tree(&src, &root);
    if let Some(o) = overlay {
        std::fs::write(root.join(".glia/overlay.toml"), o).expect("write overlay");
    }
    Fixture(root)
}

const STALE_BULLET: &str = "- `stale-anchor` (constraint, .glia/overlay.toml:20): orphaned: anchor=backend::internal::gone::Thing binds no node";

/// DT.7b: a stanza the build anchored nowhere is listed under `## Rules that
/// check nothing` (HEAD: a stderr `[declared]` line only); the exit code
/// stays the violations' (1, the forbid_edge). `--strict`: an errors row,
/// exit 2, the forbid_edge section still printed.
#[test]
fn orphaned_stanza_is_listed_not_an_error() {
    let fx = subdir_fixture("orphan", None);
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 1, "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[check] rules=3 checked=3 violations=1 (forbid_edge=1 no_cycle=0) unchecked=0 errors=0 unanchored=1",
            "[check] declared not checked: orphaned=1 rejected=0 absent=0 empty_scope=0 blocking=0",
            "[check] tiers fact=1 derived=0 heuristic=0",
            "[check] gate blocking=1 advisory=0",
        ]
    );
    assert!(
        stdout.contains("- rules: 3 (checked 3, unchecked 0, errors 0, checking nothing 1)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "### handlers-no-repository (forbid_edge, .glia/overlay.toml:3) - 1 violation(s)"
        ),
        "{stdout}"
    );
    let section = &stdout[stdout
        .find("## Rules that check nothing")
        .expect("the rules that check nothing")..];
    assert!(
        section.contains(&format!(
            "{STALE_BULLET}\n  fix: drop `anchor` to place it by its `scope`"
        )),
        "{section}"
    );
    assert!(!stdout.contains("## rule errors"), "{stdout}");

    let (code, json, _) = glia(&["check", &fx.path(), "--json"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(json.trim()).expect("JSON report");
    assert_eq!(v["errors"], serde_json::json!([]), "{json}");
    assert_eq!(v["unanchored"][0]["rule_id"], "stale-anchor", "{json}");
    assert_eq!(v["unanchored"][0]["status"], "orphaned", "{json}");
    assert_eq!(v["unanchored"][0]["blocking"], false, "{json}");

    let (code, stdout, markers) = glia(&["check", &fx.path(), "--strict"]);
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(
        markers.first().map(String::as_str),
        Some(
            "[check] rules=3 checked=3 violations=1 (forbid_edge=1 no_cycle=0) unchecked=0 errors=1 unanchored=1"
        ),
        "{markers:?}"
    );
    assert!(
        stdout.contains("- rules: 3 (checked 3, unchecked 0, errors 1)\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "### handlers-no-repository (forbid_edge, .glia/overlay.toml:3) - 1 violation(s)"
        ),
        "{stdout}"
    );
    let errors = &stdout[stdout
        .find("## rule errors (not evaluated)")
        .expect("the rule errors section")..];
    assert!(
        errors.contains(
            "- `stale-anchor`: constraint .glia/overlay.toml:20: orphaned: anchor=backend::internal::gone::Thing binds no node - not evaluated\n  fix: drop `anchor`"
        ),
        "{errors}"
    );
    assert!(!stdout.contains("## Rules that check nothing"), "{stdout}");
    let (code, json, _) = glia(&["check", &fx.path(), "--json", "--strict"]);
    assert_eq!(code, 2);
    let v: serde_json::Value = serde_json::from_str(json.trim()).expect("JSON report");
    assert_eq!(v["errors"][0][0], "stale-anchor", "{json}");
}

/// DT.7b: an overlay whose only stanza anchors nowhere is not "no rules
/// declared" (HEAD printed that and exited 0 with the stanza on stderr only):
/// the stanza is listed, exit 0; `--strict` exits 2.
#[test]
fn all_orphaned_overlay_is_not_no_rules() {
    let overlay = "version = 1

[[constraint]]
id = \"stale-anchor\"
kind = \"invariant\"
anchor = \"backend::internal::gone::Thing\"
text = \"an anchor no node carries\"
";
    let fx = subdir_fixture("allorphan", Some(overlay));
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 0, "{stdout}");
    assert!(!stdout.contains("no rules declared"), "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[check] rules=0 checked=0 violations=0 (forbid_edge=0 no_cycle=0) unchecked=0 errors=0 unanchored=1",
            "[check] declared not checked: orphaned=1 rejected=0 absent=0 empty_scope=0 blocking=0",
        ]
    );
    assert!(
        stdout.contains("- rules: 0 (checked 0, unchecked 0, errors 0, checking nothing 1)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&STALE_BULLET.replace(":20)", ":3)")),
        "{stdout}"
    );
    let (code, stdout, _) = glia(&["check", &fx.path(), "--strict"]);
    assert_eq!(code, 2, "{stdout}");
    assert!(!stdout.contains("no rules declared"), "{stdout}");
    assert!(
        stdout.contains("- `stale-anchor`: constraint .glia/overlay.toml:3: orphaned: anchor=backend::internal::gone::Thing binds no node - not evaluated"),
        "{stdout}"
    );
}

/// DT.7c fx-v1: a version-1 file whose second [[constraint]] sets the
/// version-2 key `transitive`; the overlay loader drops that stanza.
const FX_V1: &str = "version = 1

[[constraint]]
id = \"handlers-no-repository\"
kind = \"forbid_edge\"
from = \"backend/internal/handlers\"
to = \"backend/internal/repository\"
categories = [\"CALLS\"]

[[constraint]]
id = \"frontend-no-backend\"
kind = \"forbid_edge\"
from = \"frontend\"
to = \"backend\"
transitive = true
";

/// DT.7c: a stanza the overlay loader drops is listed under "Rules that
/// check nothing" with the loader's message and a fix, not an error.
/// Pre-fix (HEAD 03c30213, target/debug/glia): exit 1, `[check] rules=1 ..
/// errors=0` and no row (the stanza only on stderr's `[overlay] error:`
/// line); `--strict` exit 1.
#[test]
fn loader_rejected_stanza_is_listed_not_an_error() {
    let fx = subdir_fixture("loader-v1", Some(FX_V1));
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 1, "{stdout}");
    assert_eq!(
        markers,
        vec![
            format!(
                "[check] overlay rejected root={} whole_file=false constraint=1 component=0 layer=0",
                fx.path()
            ),
            "[check] rules=1 checked=1 violations=1 (forbid_edge=1 no_cycle=0) unchecked=0 errors=0 unanchored=1".to_string(),
            "[check] declared not checked: orphaned=0 rejected=1 absent=0 empty_scope=0 blocking=0".to_string(),
            "[check] tiers fact=1 derived=0 heuristic=0".to_string(),
            "[check] gate blocking=1 advisory=0".to_string(),
        ]
    );
    assert!(
        stdout.contains("- rules: 1 (checked 1, unchecked 0, errors 0, checking nothing 1)"),
        "{stdout}"
    );
    let section = &stdout[stdout
        .find("## Rules that check nothing")
        .expect("the rules that check nothing")..];
    assert!(
        section.contains(
            "- `frontend-no-backend` (constraint, .glia/overlay.toml:10): rejected: `transitive` need `version = 2` (this file says version = 1) (stanza dropped)\n  fix: set `version = 2` at the top of .glia/overlay.toml (version 2 loads every version-1 stanza as before), or remove the version-2 keys from this stanza\n"
        ),
        "{section}"
    );
    assert!(!stdout.contains("## rule errors"), "{stdout}");

    let (code, json, _) = glia(&["check", &fx.path(), "--json"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(json.trim()).expect("JSON report");
    assert_eq!(v["errors"], serde_json::json!([]), "{json}");
    let row = &v["unanchored"][0];
    assert_eq!(row["rule_id"], "frontend-no-backend", "{json}");
    assert_eq!(row["section"], "constraint", "{json}");
    assert_eq!(row["decl"], ".glia/overlay.toml:10", "{json}");
    assert_eq!(row["status"], "rejected", "{json}");
    assert_eq!(row["blocking"], false, "{json}");

    let (code, stdout, _) = glia(&["check", &fx.path(), "--strict"]);
    assert_eq!(code, 2, "{stdout}");
    let errors = &stdout[stdout
        .find("## rule errors (not evaluated)")
        .expect("the rule errors section")..];
    assert!(
        errors.contains(
            "- `frontend-no-backend`: constraint .glia/overlay.toml:10: rejected: `transitive` need `version = 2` (this file says version = 1) (stanza dropped) - not evaluated\n  fix: set `version = 2`"
        ),
        "{errors}"
    );
    assert!(!stdout.contains("## Rules that check nothing"), "{stdout}");
}

/// DT.7c fx-file: one misspelt key (`categorys`, line 8) ignores the whole
/// file, which holds a rule stanza: one row, not "no rules declared".
/// Pre-fix: "no rules declared", exit 0, `--strict` exit 0.
#[test]
fn whole_file_rejection_is_not_no_rules() {
    let overlay = "version = 2

[[constraint]]
id = \"handlers-no-repository\"
kind = \"forbid_edge\"
from = \"backend/internal/handlers\"
to = \"backend/internal/repository\"
categorys = [\"CALLS\"]
";
    let fx = subdir_fixture("loader-file", Some(overlay));
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 0, "{stdout}");
    assert!(!stdout.contains("no rules declared"), "{stdout}");
    assert_eq!(
        markers,
        vec![
            format!(
                "[check] overlay rejected root={} whole_file=true constraint=1 component=0 layer=0",
                fx.path()
            ),
            "[check] rules=0 checked=0 violations=0 (forbid_edge=0 no_cycle=0) unchecked=0 errors=0 unanchored=1".to_string(),
            "[check] declared not checked: orphaned=0 rejected=1 absent=0 empty_scope=0 blocking=0".to_string(),
        ]
    );
    for want in [
        "- rules: 0 (checked 0, unchecked 0, errors 0, checking nothing 1)\n",
        "- violations: 0 (blocking 0, advisory 0)\n",
        "_(no violations)_",
        "- `.glia/overlay.toml` (overlay, .glia/overlay.toml:8): rejected: unknown field `categorys`, expected one of `id`, `kind`, `from`, `to`, `categories`, `scope`, `anchor`, `text`, `origin`, `rule`, `transitive`, `via`, `min_tier`; the whole file is ignored (rule stanzas not read: constraint=1 component=0 layer=0)\n  fix: correct or remove the key the message names: every section rejects unknown keys, so one misspelt key ignores the whole file (docs/overlay.md lists each section's keys)\n",
    ] {
        assert!(stdout.contains(want), "missing {want:?} in:\n{stdout}");
    }
    let (code, stdout, _) = glia(&["check", &fx.path(), "--strict"]);
    assert_eq!(code, 2, "{stdout}");
    assert!(
        stdout.contains("- rules: 0 (checked 0, unchecked 0, errors 1)\n"),
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
            "[check] gate blocking=1 advisory=0",
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

/// The advisory mark a non-blocking violation's tier line carries (DB.3d).
const ADVISORY: &str = "advisory (below this rule's min_tier; does not fail CI)";

/// DB.3a's tree (engine/tests/check_rule_invariants.rs) beyond the two
/// manifests [`Fixture`] writes: root, store/ and scripts/ projects;
/// web/admin.py calls store/db.py's `load` (a FACT CALLS, line 5),
/// web/client.py `requests.get`s the `/orders` route services/api/server.py
/// serves (a resolver-paired, DERIVED HTTP_CALLS), handlers.py's `unused` is
/// called by nothing and scripts/tool.py's `run` calls nothing.
const DB3A_SOURCES: [(&str, &str); 10] = [
    ROOT_MANIFEST,
    ("store/pyproject.toml", "[project]\nname = \"store\"\n"),
    ("scripts/pyproject.toml", "[project]\nname = \"scripts\"\n"),
    (
        "web/app.py",
        "from services.api.handlers import get_order\n\n\ndef show(o):\n    return get_order(o)\n",
    ),
    (
        "web/admin.py",
        "from store.db import load\n\n\ndef audit(k):\n    return load(k)\n",
    ),
    (
        "web/client.py",
        "import requests\n\n\ndef orders():\n    return requests.get(\"http://api/orders\")\n",
    ),
    (
        "services/api/handlers.py",
        "from store.db import load\n\n\ndef get_order(o):\n    return load(o)\n\n\ndef unused():\n    return 1\n",
    ),
    (
        "services/api/server.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.get(\"/orders\")\ndef list_orders():\n    return []\n",
    ),
    ("store/db.py", "def load(k):\n    return k\n"),
    ("scripts/tool.py", "def run():\n    return 0\n"),
];

fn db3a_fixture(tag: &str, overlay: &str) -> Fixture {
    let files: Vec<(&str, &str)> = DB3A_SOURCES
        .iter()
        .copied()
        .chain(std::iter::once((".glia/overlay.toml", overlay)))
        .collect();
    Fixture::new(tag, &files)
}

/// The declared edge that is scripts/'s only way into store/: HEURISTIC.
const SCRIPTS_EDGE: &str = "
[[edge]]
from = \"scripts::tool::run\"
to = \"store::db::load\"
category = \"CALLS\"
";

/// The rule invariants DB.3a evaluates: one fact row with a CALLS witness,
/// one negated row with no witness and the CALLS caveats.
const RULE_INVARIANTS: &str = r#"version = 2

[[constraint]]
id = "web-no-store"
kind = "invariant"
text = "web never calls the store directly"
rule = "violation(a, b) :- calls(a, b), in_scope(a, \"web\"), in_scope(b, \"store\")."

[[constraint]]
id = "api-called"
kind = "invariant"
text = "every api handler is called"
rule = "violation(f) :- node(f, \"FUNCTION\"), in_scope(f, \"services/api/handlers.py\"), !called(f). called(f) :- edge(_, f, \"CALLS\")."
"#;

/// (1) A rule invariant's row is a located violation with its witness and
/// caveats, never a cycle; a fact row blocks. HEAD (DB.3a) printed each row
/// as `_cycle <k>: <n> node(s) in the component` with no subject.
#[test]
fn rule_violation_section_and_exit_1() {
    let fx = db3a_fixture("rule-section", RULE_INVARIANTS);
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 1, "{stdout}");
    assert_eq!(
        markers.iter().take(3).collect::<Vec<_>>(),
        vec![
            "[check] rules=2 checked=2 violations=2 (forbid_edge=0 no_cycle=0) unchecked=0 errors=0",
            "[check] tiers fact=2 derived=0 heuristic=0",
            "[check] gate blocking=2 advisory=0",
        ]
    );
    for want in [
        "- violations: 2 (blocking 2, advisory 0)",
        "### web-no-store (rule, .glia/overlay.toml:3) - 1 violation(s)",
        "_violation 1: `web::admin::audit`, `store::db::load` - tier fact at web/admin.py:4_",
        "| 1 | CALLS | `web::admin::audit` | `store::db::load` | web/admin.py:5 | graph:calls | fact |",
        "### api-called (rule, .glia/overlay.toml:9) - 1 violation(s)",
        "_violation 1: `services::api::handlers::unused` - tier fact at services/api/handlers.py:8_",
    ] {
        assert!(stdout.contains(want), "missing {want:?}:\n{stdout}");
    }
    assert!(
        !stdout.contains("_cycle") && !stdout.contains(ADVISORY),
        "a rule row is no cycle, and a fact row blocks: {stdout}"
    );
    // The negated row: no witness table, its Q5 caveats in the absence style.
    let section = &stdout[stdout
        .find("### api-called")
        .expect("the api-called section")..];
    let section = &section[..section.find("### web-no-store").unwrap_or(section.len())];
    assert!(
        !section.contains("| # | category |"),
        "a row no edge derives has no witness table: {section}"
    );
    let caveats = section.lines().filter(|l| l.starts_with("  ⚠ ")).count();
    assert!(
        (1..=5).contains(&caveats),
        "the negation read CALLS: at most 5 caveats per row: {section}"
    );
    assert!(
        section
            .lines()
            .filter(|l| l.starts_with("  ⚠ "))
            .all(
                |l| (l.starts_with("  ⚠ CALLS (") || l.starts_with("  ⚠ * (")) && l.contains("): ")
            ),
        "`  ⚠ <category> (<language>): <note>`, the rows for CALLS: {section}"
    );
    // DT.8a: one rule printed a caveat block; its one row folds nothing.
    assert_eq!(
        markers.get(3..),
        Some(&[format!("[check] caveats rules=1 lines={caveats} folded=0")][..]),
        "{markers:?}"
    );
}

/// (2) Only a heuristic violation: reported with the advisory mark, exit 0.
/// HEAD exited 1 on it, and on the version-1 forbid_edge broken only by the
/// same declared edge.
#[test]
fn advisory_only_exits_0() {
    let overlay = format!(
        "version = 2
{SCRIPTS_EDGE}
[[constraint]]
id = \"scripts-no-store\"
kind = \"invariant\"
text = \"scripts never call the store\"
rule = \"violation(a, b) :- calls(a, b), in_scope(a, \\\"scripts\\\"), in_scope(b, \\\"store\\\").\"
"
    );
    let fx = db3a_fixture("advisory-rule", &overlay);
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 0, "a heuristic violation never fails CI: {stdout}");
    assert_eq!(
        markers.last().map(String::as_str),
        Some("[check] gate blocking=0 advisory=1"),
        "{markers:?}"
    );
    for want in [
        "- violations: 1 (blocking 0, advisory 1)".to_string(),
        format!(
            "_violation 1: `scripts::tool::run`, `store::db::load` - tier heuristic, {ADVISORY} at scripts/tool.py:1_"
        ),
        "| 1 | CALLS | `scripts::tool::run` | `store::db::load` |".to_string(),
        "| overlay:edge | heuristic".to_string(),
    ] {
        assert!(stdout.contains(&want), "missing {want:?}:\n{stdout}");
    }

    // A version-1 forbid_edge broken only by the declared edge: advisory too.
    let overlay = format!(
        "version = 1
{SCRIPTS_EDGE}
[[constraint]]
id = \"scripts-direct\"
kind = \"forbid_edge\"
from = \"scripts\"
to = \"store\"
"
    );
    let fx = db3a_fixture("advisory-forbid", &overlay);
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        markers.last().map(String::as_str),
        Some("[check] gate blocking=0 advisory=1"),
        "{markers:?}"
    );
    for want in [
        "### scripts-direct (forbid_edge, .glia/overlay.toml:8) - 1 violation(s)".to_string(),
        format!("_1 forbidden edge(s), tier heuristic, {ADVISORY}:_"),
        "- violations: 1 (blocking 0, advisory 1)".to_string(),
    ] {
        assert!(stdout.contains(&want), "missing {want:?}:\n{stdout}");
    }
}

/// web/client.py `requests.get`s the `/orders` route services/api/server.py
/// serves: the forbidden edge is a resolver-paired, DERIVED HTTP_CALLS.
fn http_fixture(tag: &str, min_tier: &str) -> Fixture {
    let overlay = format!(
        "version = 2

[[constraint]]
id = \"web-no-api-http\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
categories = [\"HTTP_CALLS\"]
{min_tier}"
    );
    Fixture::new(
        tag,
        &[
            ("web/client.py", DB3A_SOURCES[5].1),
            ("services/api/server.py", DB3A_SOURCES[7].1),
            (".glia/overlay.toml", &overlay),
        ],
    )
}

/// (2b) `min_tier = "fact"` makes a derived violation advisory: exit 0 and
/// the mark on its tier line. The same rule at the default gate blocks. HEAD
/// exited 1 for both.
#[test]
fn min_tier_fact_derived_is_advisory() {
    let fx = http_fixture("min-tier-fact", "min_tier = \"fact\"\n");
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        markers.last().map(String::as_str),
        Some("[check] gate blocking=0 advisory=1"),
        "{markers:?}"
    );
    for want in [
        format!("_1 forbidden edge(s), tier derived, {ADVISORY}:_"),
        "| 1 | HTTP_CALLS | `endpoint:GET:/orders @web` | `GET /orders @services/api` | web/client.py:5 | resolver:http | derived |".to_string(),
        "- violations: 1 (blocking 0, advisory 1)".to_string(),
    ] {
        assert!(stdout.contains(&want), "missing {want:?}:\n{stdout}");
    }

    let fx = http_fixture("min-tier-default", "");
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 1, "the default min_tier is derived: {stdout}");
    assert_eq!(
        markers.last().map(String::as_str),
        Some("[check] gate blocking=1 advisory=0"),
        "{markers:?}"
    );
    for want in [
        "_1 forbidden edge(s), tier derived:_",
        "- violations: 1 (blocking 1, advisory 0)",
    ] {
        assert!(stdout.contains(want), "missing {want:?}:\n{stdout}");
    }
    assert!(!stdout.contains(ADVISORY), "{stdout}");
}

/// (3) DB.3b's fx1 with its via rule: one `_path` per hit of `to`, its
/// witness hops in path order. HEAD printed each hit as a cycle.
#[test]
fn forbid_path_section() {
    let overlay = "version = 2

[[constraint]]
id = \"web-no-api\"
kind = \"forbid_edge\"
from = \"web\"
to = \"services/api\"
transitive = true
via = [\"services/api/public\"]
";
    let fx = Fixture::new(
        "forbid-path",
        &[
            ("shared/pyproject.toml", "[project]\nname = \"shared\"\n"),
            (
                "web/app.py",
                "from shared.util import helper\nfrom services.api.public.facade import get\n\n\ndef page():\n    return helper()\n\n\ndef other():\n    return get()\n",
            ),
            (
                "shared/util.py",
                "from services.api.internal.db import query\n\n\ndef helper():\n    return query()\n",
            ),
            (
                "services/api/public/facade.py",
                "from services.api.internal.db import write\n\n\ndef get():\n    return write()\n",
            ),
            (
                "services/api/internal/db.py",
                "def query():\n    return 1\n\n\ndef write():\n    return 2\n",
            ),
            (".glia/overlay.toml", overlay),
        ],
    );
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 1, "{stdout}");
    assert_eq!(
        markers,
        vec![
            "[check] rules=1 checked=1 violations=2 (forbid_edge=0 no_cycle=0) unchecked=0 errors=0",
            "[check] tiers fact=2 derived=0 heuristic=0",
            "[check] gate blocking=2 advisory=0",
        ]
    );
    assert!(
        stdout.contains("### web-no-api (forbid_path, .glia/overlay.toml:3) - 2 violation(s)"),
        "{stdout}"
    );
    assert!(!stdout.contains("_cycle"), "a path is no cycle: {stdout}");
    // Each witness in path order, right under its own heading.
    let in_order = |lines: &[&str]| {
        let mut from = 0;
        for l in lines {
            match stdout[from..].find(l) {
                Some(i) => from += i + l.len(),
                None => panic!("missing {l:?} after byte {from}:\n{stdout}"),
            }
        }
    };
    in_order(&[
        "_path 1 into `services::api::internal::db` - 2 hop(s), tier fact:_",
        "| 1 | IMPORTS | `web::app` | `shared::util` | web/app.py:1 | graph:imports | fact |",
        "| 2 | IMPORTS | `shared::util` | `services::api::internal::db` | shared/util.py:1 | graph:imports | fact |",
        "_path 2 into `services::api::internal::db::query` - 2 hop(s), tier fact:_",
        "| 1 | CALLS | `web::app::page` | `shared::util::helper` | web/app.py:6 | graph:calls | fact |",
        "| 2 | CALLS | `shared::util::helper` | `services::api::internal::db::query` | shared/util.py:5 | graph:calls | fact |",
    ]);
}

/// DB3A's handlers.py with three functions nothing calls (`get_order` is
/// still web/app.py's callee).
const THREE_UNCALLED: &str = "from store.db import load\n\n\ndef get_order(o):\n    return load(o)\n\n\ndef unused_a():\n    return 1\n\n\ndef unused_b():\n    return 2\n\n\ndef unused_c():\n    return 3\n";

/// The line a rule's caveat block opens with (DT.8a).
const CAVEAT_BLOCK: &str = "_coverage caveats (the categories this rule's negations read; a blind extraction looks like a missing edge):_";

/// (DT.8a) A rule's coverage caveats print once, in one block under its
/// heading before its first row, never under each row; `--json` keeps every
/// row's list. HEAD printed the same `⚠` lines under each of the three rows.
#[test]
fn caveats_print_once_per_rule() {
    let overlay = r#"version = 2

[[constraint]]
id = "api-called"
kind = "invariant"
text = "every api handler is called"
rule = "violation(f) :- node(f, \"FUNCTION\"), in_scope(f, \"services/api/handlers.py\"), !called(f). called(f) :- edge(_, f, \"CALLS\")."
"#;
    let files: Vec<(&str, &str)> = DB3A_SOURCES
        .iter()
        .map(|&(p, t)| {
            if p == "services/api/handlers.py" {
                (p, THREE_UNCALLED)
            } else {
                (p, t)
            }
        })
        .chain(std::iter::once((".glia/overlay.toml", overlay)))
        .collect();
    let fx = Fixture::new("caveats-once", &files);
    let (code, stdout, markers) = glia(&["check", &fx.path()]);
    assert_eq!(code, 1, "{stdout}");
    let heading = "### api-called (rule, .glia/overlay.toml:3) - 3 violation(s)";
    let section = &stdout[stdout
        .find(heading)
        .unwrap_or_else(|| panic!("missing {heading:?}:\n{stdout}"))..];
    for k in 1..=3 {
        assert!(
            section.contains(&format!(
                "_violation {k}: `services::api::handlers::unused_"
            )),
            "row {k}: {section}"
        );
    }
    let first_row = section.find("_violation 1: ").expect("row 1");
    let caveats: Vec<&str> = section.lines().filter(|l| l.starts_with("  ⚠ ")).collect();
    assert!(!caveats.is_empty(), "the negation reads CALLS: {section}");
    for c in &caveats {
        assert_eq!(
            caveats.iter().filter(|o| *o == c).count(),
            1,
            "each caveat line once per rule, {c:?}: {section}"
        );
    }
    let head = &section[..first_row];
    assert_eq!(
        head.lines().filter(|l| l.starts_with("  ⚠ ")).count(),
        caveats.len(),
        "every caveat line under the heading, before the first row: {section}"
    );
    assert!(head.contains(CAVEAT_BLOCK), "{section}");
    // The per-row layout printed `l` lines under each of the 3 rows.
    let l = caveats.len();
    let marker = format!("[check] caveats rules=1 lines={l} folded={}", 2 * l);
    assert_eq!(
        markers.last().map(String::as_str),
        Some(marker.as_str()),
        "{markers:?}"
    );

    let (code, json, json_markers) = glia(&["check", &fx.path(), "--json"]);
    assert_eq!(code, 1);
    assert!(
        !json_markers
            .iter()
            .any(|m| m.starts_with("[check] caveats")),
        "the caveat fold is markdown only: {json_markers:?}"
    );
    let v: serde_json::Value = serde_json::from_str(json.trim()).expect("JSON report");
    let rows = v["violations"].as_array().expect("violations");
    assert_eq!(rows.len(), 3, "{json}");
    for r in rows {
        assert!(
            r["caveats"].as_array().is_some_and(|c| !c.is_empty()),
            "--json keeps every row's caveats: {r}"
        );
        assert_eq!(r["caveats"], rows[0]["caveats"], "one rule, one negation");
    }
    // The block is the rows' caveats, deduplicated, at most 5.
    let mut want: Vec<String> = Vec::new();
    for c in rows[0]["caveats"].as_array().expect("caveats") {
        let line = format!(
            "  ⚠ {} ({}): {}",
            c["edge_category"].as_str().unwrap_or_default(),
            c["language"].as_str().unwrap_or_default(),
            c["note"].as_str().unwrap_or_default()
        );
        if !want.contains(&line) {
            want.push(line);
        }
    }
    want.truncate(5);
    assert_eq!(caveats, want);
}

/// More than 50 violations in one rule: the first 50 and a note; `--json`
/// lists all.
#[test]
fn a_rule_prints_at_most_50_violations() {
    let many: String = (1..=51)
        .map(|i| format!("def f{i}():\n    return {i}\n\n\n"))
        .collect();
    let overlay = r#"version = 2

[[constraint]]
id = "no-functions"
kind = "invariant"
text = "many.py defines nothing"
rule = "violation(f) :- node(f, \"FUNCTION\"), in_scope(f, \"services/api/many.py\")."
"#;
    let fx = Fixture::new(
        "capped",
        &[
            ROOT_MANIFEST,
            ("services/api/many.py", &many),
            (".glia/overlay.toml", overlay),
        ],
    );
    let (code, stdout, _) = glia(&["check", &fx.path()]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stdout.contains("### no-functions (rule, .glia/overlay.toml:3) - 51 violation(s)"),
        "{stdout}"
    );
    assert_eq!(
        stdout
            .lines()
            .filter(|l| l.starts_with("_violation "))
            .count(),
        50,
        "{stdout}"
    );
    assert!(
        stdout.contains("_violation 50: ") && !stdout.contains("_violation 51: "),
        "{stdout}"
    );
    assert!(
        stdout.contains("_(showing 50 of 51; --json lists all)_"),
        "{stdout}"
    );
    let (code, json, _) = glia(&["check", &fx.path(), "--json"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(json.trim()).expect("JSON report");
    assert_eq!(
        v["violations"].as_array().map(Vec::len),
        Some(51),
        "--json lists all"
    );
}
