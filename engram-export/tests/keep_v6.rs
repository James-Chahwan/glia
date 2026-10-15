//! DE.6: what Engram asked glia to keep, pinned in one place. Engram's
//! flat-store result (full-text embedding beat name-only, 78 vs 54 of 100)
//! rests on three exporter behaviours:
//!
//! 1. **Leading docs reach `Content::Symbol.doc` in every language.** Pinned
//!    here end to end: `leading_doc_reaches_symbol_doc_in_every_language`
//!    builds real sources in eight languages through the bin and decodes the
//!    gmap. Each parser's DOC-cell unit tests pin the extraction; the
//!    exporter's own `tests::doc_cell_clean_and_cap` (src/lib.rs) pins the
//!    DOC-cell clean-up and its `DOC_MAX` 500-byte cap.
//! 2. **The 500-byte `cap_prose` on doc-section Propositions.** Pinned at
//!    unit level by `tests::doc_proposition_text_is_capped_at_export`
//!    (src/lib.rs: an 800+ byte section, the exact cut) and here end to end
//!    by `proposition_cap_is_500_bytes_end_to_end` (a real README through
//!    glia's doc ingest).
//! 3. **`--since` commit -> changed-symbol accuracy (CK.2).** Pinned by
//!    src/diff.rs `tests::mounted_routes_pair_by_path_not_ordinal`,
//!    `tests::hint_pass_refuses_unalike_routes`,
//!    `tests::ambiguous_route_tails_fall_back_to_the_guarded_hint` and
//!    `tests::moved_file_with_an_insert_pairs_by_name`, and by
//!    tests/since_v6.rs `since_writes_the_diff_and_seeds_file_ids` (its
//!    `B_COUNTS` added / removed / modified / moved counts and the exact
//!    modified-key map).
//!
//! The doc test is a containment check: a cleaning change that keeps the
//! phrase (C#'s `<summary>` tags, NatSpec's tag map) passes, a lost doc fails
//! and names its language. Every language is checked before the test fails,
//! so one run lists every language that lost its doc.
//!
//! Both tests run the built bin on a scratch dir under `temp_dir()` with
//! `--no-persist`, the export written beside the repo, never inside it.

use std::path::PathBuf;
use std::process::Command;

use engram_core::{Content, Gmap};

/// One documented symbol per language: (language, file, source, the
/// symbol's name, the phrase its `Symbol.doc` must contain).
const LANGS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "Python",
        "svc/orders.py",
        "def place(sku):\n    \"\"\"Place one order.\"\"\"\n    return sku\n",
        "place",
        "Place one order",
    ),
    (
        "Go",
        "pay/pay.go",
        "package pay\n\n// Charge takes the payment.\nfunc Charge() {}\n",
        "Charge",
        "Charge takes the payment",
    ),
    (
        "TypeScript",
        "web/cart.ts",
        "/** Adds an item to the cart. */\nexport function addItem() {}\n",
        "addItem",
        "Adds an item to the cart",
    ),
    (
        "Rust",
        "lines/src/lib.rs",
        "/// Parses a line.\npub fn parse_line() {}\n",
        "parse_line",
        "Parses a line",
    ),
    (
        "Java",
        "ship/Svc.java",
        "package ship;\n\npublic class Svc {\n    /** Ships an order. */\n    public void ship() {}\n}\n",
        "ship",
        "Ships an order",
    ),
    (
        "Dart",
        "app/lib/money.dart",
        "/// Formats an amount.\nString formatAmount(int cents) {\n  return cents.toString();\n}\n",
        "formatAmount",
        "Formats an amount",
    ),
    (
        "Solidity",
        "contracts/Vault.sol",
        "// SPDX-License-Identifier: MIT\npragma solidity ^0.8.0;\n\ncontract Vault {\n    \
         /// @notice Deposit ether into the vault\n    function deposit() public payable {}\n}\n",
        "deposit",
        "Deposit ether into the vault",
    ),
    (
        "C#",
        "api/Api.cs",
        "namespace Shop.Api;\n\npublic class Api\n{\n    /// <summary>Lists users.</summary>\n    \
         public string[] ListUsers()\n    {\n        return new string[0];\n    }\n}\n",
        "ListUsers",
        "Lists users",
    ),
];

/// The manifests that make each language's directory its own project.
const MANIFESTS: &[(&str, &str)] = &[
    ("pay/go.mod", "module example.com/pay\n\ngo 1.21\n"),
    (
        "lines/Cargo.toml",
        "[package]\nname = \"lines\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ),
    ("app/pubspec.yaml", "name: money\n"),
];

/// The README's `## Refunds` section: over 800 bytes, its sentences ending
/// `. ` inside the first 500 bytes.
const REFUNDS: &str = "## Refunds\n\nRefunds are the slowest part of the order lifecycle, and \
    most of the support load comes from them. A customer asks for a refund through the help \
    centre, an agent checks the order history, and the finance team approves anything above the \
    automatic threshold. The threshold is reviewed every quarter and is deliberately \
    conservative, because a refund that is issued twice is very hard to claw back once the card \
    network has settled it. Agents must never issue a refund by hand from the payment provider \
    dashboard, since that bypasses the audit trail the finance team relies on at the end of each \
    month.\n\nEvery refund goes through the order service, which records the reason and the \
    approving agent before it calls the payment provider.\n\nA refunded order keeps its shipment \
    record so that returns can still be traced.\n";

/// A scratch dir holding the repo (`fx/`) and the export (`out/`), removed
/// on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("glia-de6-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("fx")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        let s = Scratch(root);
        for (_, path, source, _, _) in LANGS {
            s.write(path, source);
        }
        for (path, body) in MANIFESTS {
            s.write(path, body);
        }
        s.write("README.md", &format!("# Shop\n\nThe shop's services.\n\n{REFUNDS}"));
        s
    }

    fn repo(&self) -> PathBuf {
        self.0.join("fx")
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.repo().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// Run the bin with `--no-persist` and decode its gmap with bincode, the
    /// way Engram reads it.
    fn export(&self) -> Gmap {
        let out = self.0.join("out").join("keep.engram-gmap");
        let run = Command::new(env!("CARGO_BIN_EXE_glia-export-engram"))
            .arg(self.repo())
            .arg("--out")
            .arg(&out)
            .arg("--no-persist")
            .env_remove("GLIA_NO_PERSIST")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert_eq!(run.status.code(), Some(0), "export failed:\n{stderr}");
        assert!(!self.repo().join(".glia").exists(), "--no-persist wrote build state");
        let gmap: Gmap = bincode::deserialize(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(gmap.format_version, engram_core::GMAP_FORMAT_VERSION);
        gmap
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every Symbol whose span lies in `path`: (key, name, doc).
fn symbols_in<'g>(gmap: &'g Gmap, path: &str) -> Vec<(&'g str, &'g str, Option<&'g str>)> {
    gmap.nodes
        .iter()
        .filter_map(|n| match &n.content {
            Content::Symbol { name, span, doc, .. }
                if gmap.files.get(&span.file).map(String::as_str) == Some(path) =>
            {
                Some((n.key.as_str(), name.as_str(), doc.as_deref()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn leading_doc_reaches_symbol_doc_in_every_language() {
    let s = Scratch::new("docs");
    let gmap = s.export();
    let mut lost = Vec::new();
    for (lang, path, _, symbol, phrase) in LANGS {
        let in_file = symbols_in(&gmap, path);
        let hits: Vec<_> = in_file.iter().filter(|(_, name, _)| name == symbol).collect();
        match hits.as_slice() {
            [(_, _, Some(doc))] if doc.contains(phrase) => {}
            [(key, _, doc)] => lost.push(format!(
                "{lang}: {key} doc {doc:?} does not contain {phrase:?}"
            )),
            _ => lost.push(format!(
                "{lang}: want one Symbol {symbol:?} in {path}, got {}; symbols there: {in_file:?}",
                hits.len()
            )),
        }
    }
    assert!(
        lost.is_empty(),
        "{} of {} languages lost a leading doc:\n  {}",
        lost.len(),
        LANGS.len(),
        lost.join("\n  ")
    );
}

#[test]
fn proposition_cap_is_500_bytes_end_to_end() {
    let s = Scratch::new("cap");
    let gmap = s.export();
    assert!(REFUNDS.len() > 800, "{}", REFUNDS.len());
    let texts: Vec<&str> = gmap
        .nodes
        .iter()
        .filter_map(|n| match &n.content {
            Content::Proposition { text, span: Some(span) }
                if gmap.files.get(&span.file).map(String::as_str) == Some("README.md") =>
            {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect();
    let refunds: Vec<&str> = texts.iter().copied().filter(|t| t.contains("Refunds are")).collect();
    let [capped] = refunds.as_slice() else {
        panic!("want one README Proposition holding the Refunds section, got {texts:?}");
    };
    assert!(capped.len() <= 500, "{} bytes: {capped:?}", capped.len());
    // A cut, not a rewrite: the section's opening bytes, ended at the last
    // sentence end inside the cap.
    let prose = REFUNDS.trim();
    assert!(prose.starts_with(capped), "not a prefix of the section: {capped:?}");
    assert!(capped.ends_with('.'), "does not end at a sentence: {capped:?}");
    assert!(
        prose[capped.len()..].starts_with(' ') || prose[capped.len()..].starts_with('\n'),
        "cut inside a sentence: {capped:?}"
    );
    assert!(
        !prose[capped.len()..500].contains(". "),
        "cut at an earlier sentence end than the last one inside 500 bytes: {capped:?}"
    );
    assert!(capped.ends_with("once the card network has settled it."), "{capped:?}");
}
