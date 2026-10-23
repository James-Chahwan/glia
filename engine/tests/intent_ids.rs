//! EB.4 — `.glia/overlay.toml [ids]`: the repo's id vocabulary. The shape
//! grammar (`{U}`, `{N}`, `{N<k>}`, `{l?}`, literals) lives once, in
//! `glia_code_domain::glia_config::IdPattern`; `glia_engine::intent::ids::IdVocab`
//! reads the config (or the defaults) and finds word-bounded id mentions.
//!
//! Run with `-- --nocapture` and grep `^\[ids\] vocab ` for the fired_on marker.

use std::path::Path;

use glia_code_domain::glia_config::{IdPattern, parse_str};
use glia_engine::intent::ids::{DEFAULT_LOCAL_WORDS, DEFAULT_PATTERNS, IdVocab, VocabSource};

fn vocab(patterns: &[&str]) -> IdVocab {
    IdVocab::from_patterns(patterns).expect("valid patterns")
}

/// (id, family) of every mention, in text order.
fn ids(v: &IdVocab, text: &str) -> Vec<(String, String)> {
    v.mentions(text).into_iter().map(|m| (m.id, m.family)).collect()
}

fn just_ids(v: &IdVocab, text: &str) -> Vec<String> {
    v.mentions(text).into_iter().map(|m| m.id).collect()
}

const DOT: &str = "{U}.{N}{l?}";

#[test]
fn grammar_accepts_and_rejects() {
    for ok in ["{U}-{N}", "T{N3}", DOT, "{U}.{N}", "FR-{N}", "PR #{N}", "{N}", "x{N9}y", "{U}{N1}"] {
        assert!(IdPattern::parse(ok).is_ok(), "{ok} should parse: {:?}", IdPattern::parse(ok));
    }
    let long = format!("{}{{N}}", "A".repeat(62));
    assert_eq!(long.chars().count(), 65);
    for (bad, needle) in [
        ("", "1..=64"),
        ("{U}-", "{N}"),
        ("TICKET", "{N}"),
        ("{X}-{N}", "unknown"),
        ("{N0}", "unknown"),
        ("{N10}", "unknown"),
        ("{u}-{N}", "unknown"),
        ("{U}-{N", "unclosed"),
        (long.as_str(), "1..=64"),
    ] {
        let e = IdPattern::parse(bad).expect_err(bad);
        assert!(e.contains(needle), "{bad}: {e}");
    }
}

#[test]
fn defaults_are_tickets_and_spec_kit_ids() {
    assert_eq!(DEFAULT_PATTERNS, ["{U}-{N}", "T{N3}"]);
    assert_eq!(DEFAULT_LOCAL_WORDS, ["analysis", "finding", "remediation", "review", "audit"]);
    let v = IdVocab::default();
    assert_eq!(v.source, VocabSource::Default);
    assert_eq!(
        ids(&v, "fix LANG-1234 and FR-001; T012 done"),
        [
            ("LANG-1234".to_string(), "LANG-".to_string()),
            ("FR-001".to_string(), "FR-".to_string()),
            ("T012".to_string(), "T".to_string()),
        ]
    );
}

#[test]
fn dotted_ids_carry_their_family() {
    let v = vocab(&[DOT]);
    assert_eq!(
        ids(&v, "DT.5h: and E0.1 then (LA.3)"),
        [
            ("DT.5h".to_string(), "DT.".to_string()),
            ("E0.1".to_string(), "E0.".to_string()),
            ("LA.3".to_string(), "LA.".to_string()),
        ]
    );
}

#[test]
fn letter_shorthands_expand() {
    let v = vocab(&[DOT]);
    assert_eq!(just_ids(&v, "LA.25a/b landed"), ["LA.25a", "LA.25b"]);
    assert_eq!(just_ids(&v, "LA.30a-c landed"), ["LA.30a", "LA.30b", "LA.30c"]);
    assert_eq!(just_ids(&v, "LA.30a\u{2013}c landed"), ["LA.30a", "LA.30b", "LA.30c"]);
    assert_eq!(just_ids(&v, "DT.5h-DT.5l"), ["DT.5h", "DT.5i", "DT.5j", "DT.5k", "DT.5l"]);
    // An extra mention sits at the shorthand's byte span.
    let m = v.mentions("LA.25a/b");
    assert_eq!((m[0].start, m[0].end), (0, 6));
    assert_eq!((m[1].start, m[1].end), (6, 8));
    // A backwards range or no letter expands nothing.
    assert_eq!(just_ids(&v, "LA.30c-a"), ["LA.30c"]);
    assert_eq!(just_ids(&v, "LA.30-c"), ["LA.30"]);
    // Only a pattern ending in {l?} expands.
    let t = vocab(&["T{N3}"]);
    assert_eq!(just_ids(&t, "T012/b T013-c"), ["T012", "T013"]);
}

#[test]
fn non_ids_are_not_found() {
    let v = IdVocab::default();
    for text in ["UTF-8x", "T12", "xT012", "0.5.2", "sha256", "T0123", "LANG-12.3"] {
        assert!(v.mentions(text).is_empty(), "{text}: {:?}", v.mentions(text));
    }
    let d = vocab(&[DOT]);
    for text in ["0.5.2", "sha256", "DT.5h.1", "dt.5h", "xDT.5h"] {
        assert!(d.mentions(text).is_empty(), "{text}: {:?}", d.mentions(text));
    }
    // A following `.` / `-` is a boundary when no digit follows it.
    assert_eq!(just_ids(&v, "see LANG-12."), ["LANG-12"]);
    assert_eq!(just_ids(&d, "DT.5h- x"), ["DT.5h"]);
    // rank_eval `dot_ids` parity: a dash then a lone later letter is a range.
    assert_eq!(just_ids(&d, "DT.5h-k"), ["DT.5h", "DT.5i", "DT.5j", "DT.5k"]);
}

#[test]
fn overlapping_patterns_pick_the_longest() {
    let v = vocab(&["FR-{N}", "{U}-{N}-{U}"]);
    assert_eq!(ids(&v, "FR-001-API"), [("FR-001-API".to_string(), "FR-".to_string())]);
    // Equal length: the earlier start wins, then the pattern order.
    let w = vocab(&["T{N3}", "{U}{N}"]);
    assert_eq!(just_ids(&w, "T012"), ["T012"]);
    assert_eq!(w.mentions("T012")[0].family, "T");
}

fn root_with(overlay: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let glia = dir.path().join(".glia");
    std::fs::create_dir_all(&glia).expect("mkdir");
    std::fs::write(glia.join("overlay.toml"), overlay).expect("write");
    dir
}

#[test]
fn config_replaces_the_defaults() {
    let dir = root_with("version = 1\n[ids]\npatterns = [\"{U}.{N}{l?}\"]\nlocal_words = [\"audit\"]\n");
    let v = IdVocab::for_root(dir.path());
    v.report("fixture", "test");
    assert_eq!(v.source, VocabSource::Config);
    assert_eq!(just_ids(&v, "DT.5h fixes LANG-1234 in T012"), ["DT.5h"]);
    assert_eq!(v.local_words, ["audit"]);

    let none = IdVocab::for_root(Path::new("/nonexistent/glia-eb4"));
    none.report("absent", "test");
    assert_eq!(none.source, VocabSource::Default);
    assert_eq!(none.local_words, DEFAULT_LOCAL_WORDS);
}

#[test]
fn empty_patterns_turn_routing_off() {
    let dir = root_with("version = 1\n[ids]\npatterns = []\n");
    let v = IdVocab::for_root(dir.path());
    v.report("fixture", "test");
    assert_eq!(v.source, VocabSource::Off);
    assert!(v.mentions("LANG-1234 T012 DT.5h").is_empty());
}

#[test]
fn a_bad_pattern_drops_only_itself() {
    let text = "version = 1\n[ids]\npatterns = [\n  \"{U}-{N}\",\n  \"{X}-{N}\",\n  \"T{N3}\",\n]\n";
    let l = parse_str(text);
    assert_eq!(l.errors.len(), 1, "{:?}", l.errors);
    assert!(l.errors[0].starts_with(".glia/overlay.toml:5: [ids] patterns"), "{}", l.errors[0]);
    assert!(l.errors[0].ends_with("(pattern dropped)"), "{}", l.errors[0]);
    let dir = root_with(text);
    let v = IdVocab::for_root(dir.path());
    v.report("fixture", "test");
    assert_eq!(v.source, VocabSource::Config);
    assert_eq!(v.rejected, 1);
    assert_eq!(v.patterns.len(), 2);
    assert_eq!(just_ids(&v, "LANG-1 T012"), ["LANG-1", "T012"]);
}

#[test]
fn trackers_and_markdown_globs_load() {
    let dir = root_with(concat!(
        "version = 1\n",
        "[ids]\n",
        "patterns = [\"{U}.{N}{l?}\"]\n",
        "markdown = [\"TODO.md\", \"dev-notes/plan-*.md\"]\n",
        "[[ids.tracker]]\n",
        "path = \"dev-notes/leap-*-packets.json\"\n",
        "list = \"packets\"\n",
        "files = \"files_touched\"\n",
    ));
    let v = IdVocab::for_root(dir.path());
    v.report("fixture", "test");
    assert_eq!(v.trackers.len(), 1);
    assert_eq!(v.trackers[0].id, "id");
    assert_eq!(v.trackers[0].list.as_deref(), Some("packets"));
    let md = v.markdown_globs().expect("markdown globs");
    assert!(md.matches("TODO.md"));
    assert!(md.matches("dev-notes/plan-0.5.3.md"));
    assert!(!md.matches("docs/README.md"));
    let trackers = v.tracker_globs(0).expect("tracker glob");
    assert!(trackers.matches("dev-notes/leap-053-packets.json"));
}
