//! HC.4 - the audited precision beside a row's tier, through the real
//! binary: `glia check`, `why`, `implementors` and `query` print it in
//! markdown (`fact (>= 0.91 on IMPORTS|fact, n=41)`, `|` escaped in a table)
//! and JSON (`"precision": {..}`), and `--no-precision` leaves it out with the
//! same exit code and otherwise the same bytes. stderr carries `[confidence]
//! surface=cli rows=<r> measured=<m> unmeasured=<u> under_floor=<f>
//! audit=<12 hex>` when a row printed one, nothing with `--no-precision`.
//! The expected numbers are read from the engine's own table
//! (`glia_engine::confidence`), so a re-audit moves no literal here.

use std::path::PathBuf;
use std::process::Command;

use glia_engine::confidence::{self, AUDIT};

/// A per-test directory under the system temp dir, removed on drop.
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str, files: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("glia-confidence-cli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in files {
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

/// `glia <args>` with persistence off: `(exit code, stdout, [confidence]
/// lines)`.
fn glia(args: &[&str]) -> (i32, String, Vec<String>) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let markers = String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("[confidence]"))
        .map(str::to_string)
        .collect::<Vec<_>>();
    for m in &markers {
        eprintln!("{m}");
    }
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        markers,
    )
}

/// `v` with every `precision` / `precision_by_tier` key removed.
fn strip(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(m) => {
            m.remove("precision");
            m.remove("precision_by_tier");
            m.values_mut().for_each(strip);
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
        _ => {}
    }
}

fn fixture(tag: &str) -> Fixture {
    Fixture::new(
        tag,
        &[
            ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
            ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
            (
                "web/app.py",
                "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n",
            ),
            ("services/api/internal.py", "def charge(o):\n    return o\n"),
            (
                ".glia/overlay.toml",
                "version = 1\n\n[[constraint]]\nid = \"web-no-api\"\nkind = \"forbid_edge\"\nfrom = \"web\"\nto = \"services/api\"\ncategories = [\"IMPORTS\"]\n",
            ),
        ],
    )
}

/// The markdown tier text `--json`'s `precision` object renders as.
fn tier_cell(tier: &str, p: &serde_json::Value) -> String {
    let mut want = confidence::Precision::unmeasured(
        p["stratum"].as_str().unwrap_or_default().to_string(),
        AUDIT,
    );
    want.unmeasured = p["unmeasured"] == true;
    want.lo = p["lo"].as_f64();
    want.n = p["n"].as_u64().unwrap_or(0);
    want.census = p["census"] == true;
    want.under_floor = p["under_floor"] == true;
    confidence::tier_text(tier, Some(&want)).replace('|', "\\|")
}

#[test]
fn check_prints_precision_beside_the_tier_and_drops_it_on_request() {
    let fx = fixture("check");
    let (code, md, markers) = glia(&["check", &fx.path()]);
    let (bare_code, bare_md, bare_markers) = glia(&["check", &fx.path(), "--no-precision"]);
    assert_eq!((code, bare_code), (1, 1), "{md}");
    assert!(bare_markers.is_empty(), "{bare_markers:?}");
    assert_eq!(markers.len(), 1, "{markers:?}");
    let m = &markers[0];
    assert!(
        m.starts_with("[confidence] surface=cli rows=") && m.ends_with(&format!("audit={AUDIT}")),
        "{m}"
    );
    // The IMPORTS row: `fact (>= .. on IMPORTS|fact.., n=..)`.
    let (_, json, _) = glia(&["check", &fx.path(), "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json).expect("JSON");
    let row = &v["violations"][0]["evidence"][0];
    let p = &row["precision"];
    assert_eq!(p["audit"], AUDIT, "{row}");
    assert!(
        p["stratum"]
            .as_str()
            .is_some_and(|s| s.starts_with("IMPORTS|fact")),
        "{row}"
    );
    let cell = tier_cell("fact", p);
    assert!(md.contains(&cell), "missing {cell:?}:\n{md}");
    assert!(!bare_md.contains("(>= "), "{bare_md}");
    // Without the text, the two reports are the same bytes.
    assert_eq!(md.replace(&cell, "fact"), bare_md, "only the precision text differs");

    let (_, bare_json, _) = glia(&["check", &fx.path(), "--json", "--no-precision"]);
    let mut stripped = v.clone();
    strip(&mut stripped);
    let bare: serde_json::Value = serde_json::from_str(&bare_json).expect("JSON");
    assert_eq!(stripped, bare, "--no-precision drops the key and nothing else");
}

#[test]
fn why_implementors_and_query_carry_it_and_drop_it() {
    let fx = fixture("why");
    let repo = fx.path();
    for args in [
        vec!["why", repo.as_str(), "web::app::pay", "services::api::internal::charge"],
        vec!["implementors", repo.as_str(), "charge"],
        vec!["query", repo.as_str(), "-e", "out(A, B) :- calls(A, B).", "--explain"],
    ] {
        let (code, json, markers) = glia(&[args.as_slice(), &["--json"]].concat());
        let (bare_code, bare_json, bare_markers) =
            glia(&[args.as_slice(), &["--json", "--no-precision"]].concat());
        assert_eq!(code, bare_code, "{args:?}");
        assert!(bare_markers.is_empty(), "{args:?}: {bare_markers:?}");
        let mut v: serde_json::Value = serde_json::from_str(&json).expect("JSON");
        let had = json.contains("\"precision\"");
        assert_eq!(had, !markers.is_empty(), "{args:?}: {markers:?}");
        strip(&mut v);
        let bare: serde_json::Value = serde_json::from_str(&bare_json).expect("JSON");
        assert_eq!(v, bare, "{args:?}: only the key differs");
    }
    let (_, md, _) = glia(&[
        "why",
        &repo,
        "web::app::pay",
        "services::api::internal::charge",
    ]);
    assert!(md.contains("fact (>= "), "{md}");
    assert!(md.contains(" on CALLS\\|fact"), "{md}");
}
