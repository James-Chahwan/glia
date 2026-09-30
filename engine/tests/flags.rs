//! CC.7b: the stale feature-flag report. The tree is the spec probe:
//! service/checkout.py reads `new-checkout` in `checkout` and `promo-banner`
//! in `banner` (LaunchDarkly `client.variation`), flipt/features.yaml defines
//! `new-checkout` and `legacy-search`, and service/promo.py reads
//! `promo-banner` a second time in `show`. So `legacy-search` is defined and
//! never read (dead), `promo-banner` is read by two functions and defined
//! nowhere (undefined), and `new-checkout` is read in one place (single-site).
//! History is written with `code_domain::snapshots::write_history` before the
//! build, no git needed.

use std::path::Path;

use glia_code_domain::snapshots::{
    BlameFile, HistoryCommit, HistoryFile, HistoryMeta, write_history,
};
use glia_engine::flags::{DEFAULT_QUIET_DAYS, FlagArgs, FlagRow, FlagSite, FlagsReport, flags};
use glia_engine::{generate_many, generate_one};
use glia_graph::MergedGraph;

const CHECKOUT_PY: &str = "import ldclient\n\nclient = ldclient.get()\n\n\ndef checkout(user):\n    if client.variation(\"new-checkout\", user, False):\n        return 1\n    return 0\n\n\ndef banner(user):\n    return client.variation(\"promo-banner\", user, False)\n";
const PROMO_PY: &str = "import ldclient\n\nclient = ldclient.get()\n\n\ndef show(user):\n    return client.variation(\"promo-banner\", user, False)\n";
const FEATURES_YAML: &str = "namespace: default\nflags:\n  - key: new-checkout\n    name: New checkout\n  - key: legacy-search\n    name: Legacy search\n";

const CHECKOUT: &str = "service/checkout.py";
const PROMO: &str = "service/promo.py";
const FEATURES: &str = "flipt/features.yaml";

const T: i64 = 1_700_000_000;
const DAY: i64 = 86_400;

fn write(root: &Path, files: &[(&str, &str)]) {
    for (p, text) in files {
        let path = root.join(p);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
}

/// The acceptance tree; `with_flipt` adds the Flipt definitions file.
fn tree(with_flipt: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    write(d.path(), &[(CHECKOUT, CHECKOUT_PY), (PROMO, PROMO_PY)]);
    if with_flipt {
        write(d.path(), &[(FEATURES, FEATURES_YAML)]);
    }
    d
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy()).expect("build").merged
}

fn commit(i: u8, t: i64, paths: &[&str]) -> HistoryCommit {
    HistoryCommit {
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
    }
}

/// `commits` newest first.
fn snapshot(root: &Path, commits: &[HistoryCommit], blame: &[BlameFile]) {
    let head = commits
        .first()
        .map_or_else(|| "0".repeat(40), |c| c.c.clone());
    write_history(
        root,
        HistoryMeta::new(head, 2000, None, String::new()),
        commits,
        blame,
    )
    .expect("write snapshot");
}

fn row<'a>(r: &'a FlagsReport, key: &str) -> &'a FlagRow {
    r.flags
        .iter()
        .find(|f| f.key == key)
        .unwrap_or_else(|| panic!("no row {key}: {r:#?}"))
}

fn keys(r: &FlagsReport) -> Vec<&str> {
    r.flags.iter().map(|f| f.key.as_str()).collect()
}

fn findings(f: &FlagRow) -> Vec<(&str, &str)> {
    f.findings.iter().map(|x| (x.status, x.tier)).collect()
}

fn sites(s: &[FlagSite]) -> Vec<(&str, &str, Option<&str>, Option<i64>)> {
    s.iter()
        .map(|x| (x.qname.as_str(), x.kind, x.file.as_deref(), x.line))
        .collect()
}

fn count(r: &FlagsReport, status: &str) -> usize {
    r.counts
        .get(status)
        .copied()
        .unwrap_or_else(|| panic!("no count {status}: {:?}", r.counts))
}

/// Every flag, its sites and its findings, with no history snapshot.
#[test]
fn inventory_and_findings() {
    let d = tree(true);
    let r = flags(&build(d.path()), &FlagArgs::default());
    assert_eq!(
        keys(&r),
        ["legacy-search", "new-checkout", "promo-banner"],
        "{r:#?}"
    );

    // Defined and never read. The Flipt scanner records the file, not the
    // `- key:` line (its DEFINES_CONFIG evidence has basis `file`), so the
    // site has no line; the yaml MODULE is the definer.
    let legacy = row(&r, "legacy-search");
    assert_eq!(findings(legacy), [("dead", "derived")]);
    assert_eq!(
        sites(&legacy.definitions),
        [("flipt::features.yaml", "MODULE", Some(FEATURES), None)]
    );
    assert!(legacy.reads.is_empty() && legacy.readers == 0);
    assert_eq!(legacy.providers, ["flipt"]);
    assert_eq!(
        legacy.findings[0].note,
        "defined in flipt/features.yaml; no literal read of the key is extracted (a key built at runtime is not captured)"
    );

    // Read in one function, at the call's line (CC.7a's EVIDENCE site).
    let checkout = row(&r, "new-checkout");
    assert_eq!(findings(checkout), [("single_site", "fact")]);
    assert_eq!(
        sites(&checkout.reads),
        [(
            "service::checkout::checkout",
            "FUNCTION",
            Some(CHECKOUT),
            Some(7)
        )]
    );
    assert_eq!(checkout.readers, 1);
    assert_eq!(
        sites(&checkout.definitions),
        [("flipt::features.yaml", "MODULE", Some(FEATURES), None)]
    );
    assert_eq!(checkout.providers, ["flipt", "launchdarkly"]);
    assert_eq!(
        checkout.findings[0].note,
        "read only in service::checkout::checkout (service/checkout.py:7)"
    );

    // Read by two functions, defined nowhere although the repo keeps a
    // definition file: undefined, and not single-site.
    let promo = row(&r, "promo-banner");
    assert_eq!(findings(promo), [("undefined", "derived")]);
    assert_eq!(
        sites(&promo.reads),
        [
            (
                "service::checkout::banner",
                "FUNCTION",
                Some(CHECKOUT),
                Some(13)
            ),
            ("service::promo::show", "FUNCTION", Some(PROMO), Some(7)),
        ]
    );
    assert_eq!(promo.readers, 2);
    assert!(promo.definitions.is_empty());
    assert_eq!(promo.providers, ["launchdarkly"]);
    assert_eq!(
        promo.findings[0].note,
        "read, but none of the 1 flag definition file(s) in the graph defines it; a provider console keeps definitions outside the repo"
    );

    assert_eq!(r.definitions_in_graph, 1);
    assert!(!r.quiet_evaluated);
    assert_eq!(r.history_now, None);
    assert_eq!(r.quiet_days, DEFAULT_QUIET_DAYS);
    assert!(r.flags.iter().all(|f| f.last_read_change.is_none()));
    assert_eq!(
        (
            count(&r, "dead"),
            count(&r, "undefined"),
            count(&r, "single_site"),
            count(&r, "quiet")
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(r.counts.len(), 4);
    assert!(r.absence.is_none());
}

/// A repo that keeps no flag definition file raises no undefined finding: a
/// provider console holds them, and the report does not guess.
#[test]
fn no_definition_files_no_undefined() {
    let d = tree(false);
    let r = flags(&build(d.path()), &FlagArgs::default());
    assert_eq!(r.definitions_in_graph, 0);
    // A row with a finding sorts before a row with none.
    assert_eq!(keys(&r), ["new-checkout", "promo-banner"], "{r:#?}");
    assert_eq!(findings(row(&r, "new-checkout")), [("single_site", "fact")]);
    assert!(row(&r, "promo-banner").findings.is_empty());
    assert_eq!(count(&r, "undefined"), 0);
    assert!(r.absence.is_none());
}

/// Quiet is measured against the snapshot's own newest change, never the
/// wall clock, and needs every reader quiet.
#[test]
fn quiet_uses_the_snapshot_clock() {
    let d = tree(true);
    let later = T + 200 * DAY;
    snapshot(
        d.path(),
        &[commit(2, later, &[PROMO]), commit(1, T, &[CHECKOUT, PROMO])],
        &[
            BlameFile {
                p: CHECKOUT.into(),
                runs: vec![[1, 13, T]],
            },
            BlameFile {
                p: PROMO.into(),
                runs: vec![[1, 7, later]],
            },
        ],
    );
    let m = build(d.path());
    let r = flags(&m, &FlagArgs::default());
    assert!(r.quiet_evaluated, "{r:#?}");
    assert_eq!(r.history_now, Some(later));

    let checkout = row(&r, "new-checkout");
    assert_eq!(
        findings(checkout),
        [("single_site", "fact"), ("quiet", "heuristic")]
    );
    assert_eq!(checkout.last_read_change, Some(T));
    assert_eq!(
        checkout.findings[1].note,
        "no reading line changed in 200 days before the snapshot's newest change (git blame, not runtime use)"
    );

    // banner's line is as old as checkout's, but show changed at `later`.
    let promo = row(&r, "promo-banner");
    assert_eq!(findings(promo), [("undefined", "derived")]);
    assert_eq!(promo.last_read_change, Some(later));

    // Dead flags have no reader, so never quiet.
    assert_eq!(findings(row(&r, "legacy-search")), [("dead", "derived")]);
    assert_eq!(count(&r, "quiet"), 1);

    // The threshold is `quiet_days`: 200 days is not quiet at 250.
    let mut args = FlagArgs::default();
    args.quiet_days = 250;
    let r = flags(&m, &args);
    assert_eq!(findings(row(&r, "new-checkout")), [("single_site", "fact")]);
    assert_eq!((r.quiet_days, count(&r, "quiet")), (250, 0));
    assert!(r.quiet_evaluated);
}

/// A reader with no history recency (its lines are outside every blame run)
/// blocks quiet, however old the other readers are, and leaves the flag's
/// last read change unknown.
#[test]
fn quiet_needs_every_reader_dated() {
    let d = tree(true);
    let later = T + 200 * DAY;
    snapshot(
        d.path(),
        &[
            commit(2, later, &[CHECKOUT]),
            commit(1, T, &[CHECKOUT, PROMO]),
        ],
        &[
            // checkout (lines 6..9) is blamed; banner (12..13) is not.
            BlameFile {
                p: CHECKOUT.into(),
                runs: vec![[1, 9, T]],
            },
            BlameFile {
                p: PROMO.into(),
                runs: vec![[1, 7, T]],
            },
        ],
    );
    let r = flags(&build(d.path()), &FlagArgs::default());
    assert_eq!(r.history_now, Some(later));
    assert_eq!(
        findings(row(&r, "new-checkout")),
        [("single_site", "fact"), ("quiet", "heuristic")]
    );
    let promo = row(&r, "promo-banner");
    assert_eq!(findings(promo), [("undefined", "derived")]);
    assert_eq!(promo.last_read_change, None);
}

/// One key read in one repo and defined in another is one row.
#[test]
fn keys_group_across_repos() {
    let svc = tempfile::tempdir().expect("tempdir");
    write(svc.path(), &[(CHECKOUT, CHECKOUT_PY)]);
    let infra = tempfile::tempdir().expect("tempdir");
    write(
        infra.path(),
        &[(
            FEATURES,
            "flags:\n  - key: promo-banner\n    name: Promo banner\n",
        )],
    );
    let repos = [
        svc.path().to_string_lossy().into_owned(),
        infra.path().to_string_lossy().into_owned(),
    ];
    let r = flags(
        &generate_many(&repos).expect("build").merged,
        &FlagArgs::default(),
    );

    assert_eq!(keys(&r), ["new-checkout", "promo-banner"], "{r:#?}");
    let promo = row(&r, "promo-banner");
    assert_eq!(findings(promo), [("single_site", "fact")]);
    assert_eq!(
        sites(&promo.definitions),
        [("flipt::features.yaml", "MODULE", Some(FEATURES), None)]
    );
    assert_eq!(
        sites(&promo.reads),
        [(
            "service::checkout::banner",
            "FUNCTION",
            Some(CHECKOUT),
            Some(13)
        )]
    );
    assert_eq!(promo.providers, ["flipt", "launchdarkly"]);
    assert_eq!(
        findings(row(&r, "new-checkout")),
        [("undefined", "derived"), ("single_site", "fact")]
    );
}

/// `scope` keeps a row when any definition or read sits under it; a scope
/// that keeps none says so instead of answering empty.
#[test]
fn scope_keeps_rows_with_a_site_under_it() {
    let d = tree(true);
    let m = build(d.path());
    let scoped = |s: &str| {
        let mut args = FlagArgs::default();
        args.scope = Some(s.into());
        flags(&m, &args)
    };
    assert_eq!(keys(&scoped("flipt")), ["legacy-search", "new-checkout"]);
    assert_eq!(keys(&scoped("service")), ["new-checkout", "promo-banner"]);
    let promo_only = scoped("service/promo.py");
    assert_eq!(keys(&promo_only), ["promo-banner"]);
    // The row stays whole: both readers, and the report-wide counts.
    assert_eq!(row(&promo_only, "promo-banner").readers, 2);
    assert_eq!(promo_only.definitions_in_graph, 1);

    let none = scoped("docs");
    assert!(none.flags.is_empty());
    assert_eq!(none.absence.as_ref().map(|a| a.reason), Some("no_match"));
}

/// No flag read or definition at all: an empty report with its reason.
#[test]
fn no_flags_absence() {
    let d = tempfile::tempdir().expect("tempdir");
    write(
        d.path(),
        &[(
            "app/main.py",
            "import os\n\n\ndef main():\n    return os.environ.get(\"HOME\")\n",
        )],
    );
    let r = flags(&build(d.path()), &FlagArgs::default());
    assert!(r.flags.is_empty());
    let a = r.absence.as_ref().expect("absence");
    assert_eq!(a.reason, "no_match");
    assert_eq!(a.mechanisms, ["READS_CONFIG", "DEFINES_CONFIG"]);
    assert!(
        a.note
            .contains("no feature-flag read or definition was extracted"),
        "{}",
        a.note
    );
    assert_eq!(r.definitions_in_graph, 0);
    assert!(r.counts.values().all(|&n| n == 0));
}

/// Two builds of one tree, and two answers over one graph, serialise alike.
#[test]
fn deterministic() {
    let json =
        |m: &MergedGraph| serde_json::to_string(&flags(m, &FlagArgs::default())).expect("json");
    let a = tree(true);
    let later = T + 200 * DAY;
    let history = |root: &Path| {
        snapshot(
            root,
            &[commit(2, later, &[PROMO]), commit(1, T, &[CHECKOUT, PROMO])],
            &[BlameFile {
                p: CHECKOUT.into(),
                runs: vec![[1, 13, T]],
            }],
        )
    };
    history(a.path());
    let first = build(a.path());
    let b = tree(true);
    history(b.path());
    let second = build(b.path());
    let once = json(&first);
    assert_eq!(once, json(&first));
    assert_eq!(once, json(&second));
}
