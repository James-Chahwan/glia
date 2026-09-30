//! CC.7c — `glia flags`, driving the real binary over CC.7b's acceptance tree
//! (engine/tests/flags.rs): service/checkout.py reads `new-checkout` in
//! `checkout` and `promo-banner` in `banner` (LaunchDarkly
//! `client.variation`), service/promo.py reads `promo-banner` again in `show`,
//! and flipt/features.yaml defines `new-checkout` and `legacy-search`. So
//! `legacy-search` is dead, `promo-banner` undefined, `new-checkout`
//! single-site. The history snapshot, when a test wants one, is written with
//! `code_domain::snapshots::write_history` before the build, no git needed.
//!
//! The engine's `[flags] keys=<K> defined_files=<D> dead=<a> undefined=<b>
//! single_site=<c> quiet=<d> quiet_evaluated=<bool>` stderr line is the
//! fired_on marker; asserting it here makes it a tested contract.

use std::path::PathBuf;
use std::process::{Command, Output};

use glia_code_domain::snapshots::{
    BlameFile, HistoryCommit, HistoryFile, HistoryMeta, write_history,
};

const CHECKOUT_PY: &str = "import ldclient\n\nclient = ldclient.get()\n\n\ndef checkout(user):\n    if client.variation(\"new-checkout\", user, False):\n        return 1\n    return 0\n\n\ndef banner(user):\n    return client.variation(\"promo-banner\", user, False)\n";
const PROMO_PY: &str = "import ldclient\n\nclient = ldclient.get()\n\n\ndef show(user):\n    return client.variation(\"promo-banner\", user, False)\n";
const FEATURES_YAML: &str = "namespace: default\nflags:\n  - key: new-checkout\n    name: New checkout\n  - key: legacy-search\n    name: Legacy search\n";

const CHECKOUT: &str = "service/checkout.py";
const PROMO: &str = "service/promo.py";
const FEATURES: &str = "flipt/features.yaml";

/// 2023-11-14; `LATER` (200 days on) is 2024-06-01.
const T: i64 = 1_700_000_000;
const LATER: i64 = T + 200 * 86_400;

const SECTION_HEAD: &str = "| flag | tier | readers | read at | defined at | note |";
const DEAD_ROW: &str = "| `legacy-search` | derived | 0 | — | flipt/features.yaml | defined in flipt/features.yaml; no literal read of the key is extracted (a key built at runtime is not captured) |";
const UNDEFINED_ROW: &str = "| `promo-banner` | derived | 2 | service/checkout.py:13 +1 more | — | read, but none of the 1 flag definition file(s) in the graph defines it; a provider console keeps definitions outside the repo |";
const SINGLE_ROW: &str = "| `new-checkout` | fact | 1 | service/checkout.py:7 | flipt/features.yaml | read only in service::checkout::checkout (service/checkout.py:7) |";
const QUIET_ROW: &str = "| `new-checkout` | heuristic | 1 | service/checkout.py:7 | flipt/features.yaml | no reading line changed in 200 days before the snapshot's newest change (git blame, not runtime use) |";

/// A fresh temp root holding `files`, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(tag: &str, files: &[(&str, &str)]) -> Self {
        let p = std::env::temp_dir().join(format!("glia-cc7c-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        for (rel, text) in files {
            let path = p.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, text).expect("write source");
        }
        Root(p)
    }

    /// The acceptance tree.
    fn tree(tag: &str) -> Self {
        Root::new(
            tag,
            &[
                (CHECKOUT, CHECKOUT_PY),
                (PROMO, PROMO_PY),
                (FEATURES, FEATURES_YAML),
            ],
        )
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("utf-8 temp path")
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Engine test `quiet_uses_the_snapshot_clock`'s history: every file at `T`,
/// promo.py again at `LATER`; checkout.py blamed at `T` throughout, promo.py
/// at `LATER`. So `new-checkout`'s one reader last changed 200 days before
/// the snapshot's newest change, and `promo-banner`'s `show` at it.
fn history(root: &Root) {
    let commit = |i: u8, t: i64, paths: &[&str]| HistoryCommit {
        c: format!("{i:02}{}", "0".repeat(38)),
        t,
        files: paths
            .iter()
            .map(|p| HistoryFile {
                p: p.to_string(),
                a: Some(1),
                d: Some(0),
                from: None,
            })
            .collect(),
    };
    let commits = [commit(2, LATER, &[PROMO]), commit(1, T, &[CHECKOUT, PROMO])];
    let blame = [
        BlameFile {
            p: CHECKOUT.into(),
            runs: vec![[1, 13, T]],
        },
        BlameFile {
            p: PROMO.into(),
            runs: vec![[1, 7, LATER]],
        },
    ];
    let meta = HistoryMeta::new(commits[0].c.clone(), 2000, None, String::new());
    write_history(&root.0, meta, &commits, &blame).expect("write snapshot");
}

/// Run `glia flags <args>`, relaying the fired_on marker, and check the exit
/// code.
fn glia(args: &[&str], want: i32) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .arg("flags")
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(want),
        "glia flags {args:?}\nstdout:\n{}\nstderr:\n{stderr}",
        stdout(&out)
    );
    // Relay the marker so `-- --nocapture | grep '^\[flags\] '` sees it.
    for line in stderr.lines().filter(|l| l.starts_with("[flags] ")) {
        eprintln!("{line}");
    }
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn markers(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("[flags] "))
        .map(str::to_string)
        .collect()
}

/// The lines under `## <title>` up to the next section, blank lines dropped.
fn section<'a>(text: &'a str, title: &str) -> Vec<&'a str> {
    let head = format!("## {title}");
    text.lines()
        .skip_while(|l| *l != head)
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| !l.is_empty())
        .collect()
}

/// The `## ` titles, in print order.
fn titles(text: &str) -> Vec<&str> {
    text.lines().filter_map(|l| l.strip_prefix("## ")).collect()
}

/// One table per finding, in STATUSES order, then the inventory; the header
/// says the quiet rule was not evaluated (no history snapshot).
#[test]
fn tables_per_finding() {
    let root = Root::tree("tables");
    let out = glia(&[root.path()], 0);
    let text = stdout(&out);
    assert!(
        text.contains(
            "- flags: 3; definition files: 1; quiet evaluated: no (run glia history sync --blame)"
        ),
        "{text}"
    );
    assert_eq!(
        titles(&text),
        ["dead", "undefined", "single_site", "quiet", "all flags"],
        "{text}"
    );
    let rule = "|---|---|--:|---|---|---|";
    assert_eq!(
        section(&text, "dead"),
        [SECTION_HEAD, rule, DEAD_ROW],
        "{text}"
    );
    assert_eq!(
        section(&text, "undefined"),
        [SECTION_HEAD, rule, UNDEFINED_ROW],
        "{text}"
    );
    assert_eq!(
        section(&text, "single_site"),
        [SECTION_HEAD, rule, SINGLE_ROW],
        "{text}"
    );
    assert_eq!(
        section(&text, "quiet"),
        ["_(not evaluated: no reader carries history; run glia history sync --blame)_"],
        "{text}"
    );
    assert_eq!(
        section(&text, "all flags"),
        [
            "| flag | providers | readers | definitions |",
            "|---|---|--:|--:|",
            "| `legacy-search` | flipt | 0 | 1 |",
            "| `new-checkout` | flipt, launchdarkly | 1 | 1 |",
            "| `promo-banner` | launchdarkly | 2 | 0 |",
        ],
        "{text}"
    );
    assert_eq!(
        markers(&out),
        [
            "[flags] keys=3 defined_files=1 dead=1 undefined=1 single_site=1 quiet=0 quiet_evaluated=false"
        ],
        "one marker per call"
    );
}

/// `--status` keeps only the listed findings' sections, in section order
/// whatever the flag order, and drops the inventory; an unknown status is a
/// usage error.
#[test]
fn status_filters_sections() {
    let root = Root::tree("status");
    let text = stdout(&glia(&[root.path(), "--status", "dead"], 0));
    assert_eq!(titles(&text), ["dead"], "{text}");
    assert_eq!(section(&text, "dead")[2..], [DEAD_ROW], "{text}");
    assert!(text.contains("- flags: 3; definition files: 1;"), "{text}");

    let text = stdout(&glia(
        &[
            root.path(),
            "--status",
            "single_site",
            "--status",
            "undefined",
        ],
        0,
    ));
    assert_eq!(titles(&text), ["undefined", "single_site"], "{text}");
    assert_eq!(section(&text, "undefined")[2..], [UNDEFINED_ROW], "{text}");
    assert_eq!(section(&text, "single_site")[2..], [SINGLE_ROW], "{text}");

    let bad = glia(&[root.path(), "--status", "stale"], 2);
    let err = String::from_utf8_lossy(&bad.stderr);
    assert!(
        err.contains("stale") && err.contains("single_site"),
        "{err}"
    );
}

/// `--json` is the engine's report in its field order; `--status` keeps the
/// rows holding a listed finding and leaves the report-wide counts alone.
#[test]
fn json_is_the_report() {
    let root = Root::tree("json");
    let out = glia(&[root.path(), "--json"], 0);
    let text = stdout(&out);
    assert!(
        text.starts_with("{\"flags\":[{\"key\":\"legacy-search\""),
        "{text}"
    );
    let v: serde_json::Value = serde_json::from_str(text.trim()).expect("json");
    let mut keys: Vec<&str> = v
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "absence",
            "counts",
            "definitions_in_graph",
            "flags",
            "history_now",
            "quiet_days",
            "quiet_evaluated"
        ],
        "{text}"
    );
    let flags: Vec<&str> = v["flags"]
        .as_array()
        .expect("flags")
        .iter()
        .filter_map(|f| f["key"].as_str())
        .collect();
    assert_eq!(flags, ["legacy-search", "new-checkout", "promo-banner"]);
    assert_eq!(v["flags"][0]["definitions"][0]["file"], FEATURES);
    assert!(v["flags"][0]["definitions"][0]["line"].is_null(), "{text}");
    assert_eq!(v["flags"][1]["reads"][0]["line"], 7);
    assert_eq!(v["definitions_in_graph"], 1);
    assert_eq!(v["quiet_evaluated"], false);
    assert!(v["history_now"].is_null() && v["absence"].is_null());
    assert_eq!(v["quiet_days"], 90);
    assert_eq!(
        v["counts"],
        serde_json::json!({"dead": 1, "quiet": 0, "single_site": 1, "undefined": 1})
    );
    assert_eq!(markers(&out).len(), 1);

    let v: serde_json::Value =
        serde_json::from_str(stdout(&glia(&[root.path(), "--json", "--status", "dead"], 0)).trim())
            .expect("json");
    assert_eq!(v["flags"].as_array().map(Vec::len), Some(1));
    assert_eq!(v["flags"][0]["key"], "legacy-search");
    assert_eq!(v["counts"]["undefined"], 1, "counts stay report-wide");
}

/// With a history snapshot the header dates it and `quiet` lists the flag
/// whose every reader is `--quiet-days` older than the snapshot's newest
/// change; past the gap, `quiet` is evaluated and empty.
#[test]
fn quiet_with_history() {
    let root = Root::tree("quiet");
    history(&root);
    let out = glia(&[root.path()], 0);
    let text = stdout(&out);
    assert!(
        text.contains(
            "- flags: 3; definition files: 1; quiet evaluated: yes (history to 2024-06-01)"
        ),
        "{text}"
    );
    assert_eq!(section(&text, "quiet")[2..], [QUIET_ROW], "{text}");
    assert_eq!(section(&text, "single_site")[2..], [SINGLE_ROW], "{text}");
    assert_eq!(
        markers(&out),
        [
            "[flags] keys=3 defined_files=1 dead=1 undefined=1 single_site=1 quiet=1 quiet_evaluated=true"
        ]
    );

    let out = glia(
        &[root.path(), "--quiet-days", "250", "--status", "quiet"],
        0,
    );
    let text = stdout(&out);
    assert_eq!(titles(&text), ["quiet"], "{text}");
    assert_eq!(section(&text, "quiet"), ["_(none)_"], "{text}");
    assert!(
        markers(&out)[0].ends_with(" quiet=0 quiet_evaluated=true"),
        "{:?}",
        markers(&out)
    );
}

/// `--scope` keeps a flag with any site under it; `--with` groups one key
/// read in one repo and defined in another into one row.
#[test]
fn scope_and_with() {
    let root = Root::tree("scope");
    let text = stdout(&glia(&[root.path(), "--scope", "flipt"], 0));
    assert!(text.contains("- flags: 2; definition files: 1;"), "{text}");
    assert_eq!(
        section(&text, "all flags")[2..],
        [
            "| `legacy-search` | flipt | 0 | 1 |",
            "| `new-checkout` | flipt, launchdarkly | 1 | 1 |"
        ],
        "{text}"
    );

    let svc = Root::new("with-svc", &[(CHECKOUT, CHECKOUT_PY)]);
    let infra = Root::new(
        "with-infra",
        &[(
            FEATURES,
            "flags:\n  - key: promo-banner\n    name: Promo banner\n",
        )],
    );
    // new-checkout is read once too, and defined in neither repo: undefined
    // (infra holds a definition file) and single-site.
    let text = stdout(&glia(&[svc.path(), "--with", infra.path()], 0));
    assert_eq!(
        section(&text, "single_site")[2..],
        [
            "| `new-checkout` | fact | 1 | service/checkout.py:7 | — | read only in service::checkout::checkout (service/checkout.py:7) |",
            "| `promo-banner` | fact | 1 | service/checkout.py:13 | flipt/features.yaml | read only in service::checkout::banner (service/checkout.py:13) |",
        ],
        "{text}"
    );
    assert_eq!(
        section(&text, "all flags")[2..],
        [
            "| `new-checkout` | launchdarkly | 1 | 0 |",
            "| `promo-banner` | flipt, launchdarkly | 1 | 1 |",
        ],
        "{text}"
    );
}

/// No flag at all is a report too: exit 0, and the absence says why. A path
/// that does not build exits 2.
#[test]
fn empty_report_and_build_error() {
    let root = Root::new(
        "empty",
        &[(
            "app/main.py",
            "import os\n\n\ndef main():\n    return os.environ.get(\"HOME\")\n",
        )],
    );
    let out = glia(&[root.path()], 0);
    let text = stdout(&out);
    assert!(text.contains("- flags: 0; definition files: 0;"), "{text}");
    assert!(text.contains("_(no flags)_"), "{text}");
    assert!(
        text.contains("> FACT: no feature-flag read or definition was extracted"),
        "{text}"
    );
    assert!(titles(&text).is_empty(), "{text}");
    assert_eq!(
        markers(&out),
        [
            "[flags] keys=0 defined_files=0 dead=0 undefined=0 single_site=0 quiet=0 quiet_evaluated=false"
        ]
    );

    let missing =
        std::env::temp_dir().join(format!("glia-cc7c-cli-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    glia(&[missing.to_str().expect("utf-8 temp path")], 2);
}
