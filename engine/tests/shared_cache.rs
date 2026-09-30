//! CE.2a gate: the shared parse cache's engine half, keys and export.
//!
//! A cached parse is addressed by a blake3 key over every input the build's
//! language branch reads (build stamp, repo identity, language, path, MODULE
//! form, the go.mod set for a Go file, the content), and a checkout exports the
//! sidecar entries a build of it would reuse, each as the entry's own bytes.
//! The fixture repo is a.py, b.go + go.mod and the same-stem pair util.ts +
//! util.js (LB.13 names both MODULEs by file name), so the Go context and the
//! module form are both exercised.

use std::collections::BTreeMap;
use std::path::Path;

use glia_code_domain::FileParse;
use glia_code_domain::walk_gating::repo_identity;
use glia_engine::shared_cache::{CacheKey, CacheRows, cache_rows, export_entries, file_key};
use glia_engine::{BUILD_STAMP, GoModules, generate_one_incremental};

const FILES: &[(&str, &str)] = &[
    ("a.py", "def a():\n    return 1\n"),
    ("go.mod", "module example.com/b\n\ngo 1.21\n"),
    ("b.go", "package main\n\nfunc B() int { return 2 }\n"),
    (
        "util.ts",
        "export function util(): number {\n  return 1;\n}\n",
    ),
    (
        "util.js",
        "function utilJs() {\n  return 2;\n}\nmodule.exports = { utilJs };\n",
    ),
];

/// The four files a build hands a language parser, in path order, with the
/// MODULE qname each gets (util.ts + util.js are a same-group stem).
const PARSED: &[(&str, &str, &str)] = &[
    ("a.py", "python", "a"),
    ("b.go", "go", "b"),
    ("util.js", "typescript", "util.js"),
    ("util.ts", "typescript", "util.ts"),
];

fn write_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create repo dir");
    for (name, text) in FILES {
        std::fs::write(dir.join(name), text).expect("write fixture file");
    }
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 temp path")
}

/// A built fixture repo at `<tmp>/<name>`: its sidecar is on disk.
fn built(tmp: &Path, name: &str) -> std::path::PathBuf {
    let repo = tmp.join(name);
    write_repo(&repo);
    generate_one_incremental(s(&repo)).expect("incremental build");
    assert!(
        sidecar_path(&repo).is_file(),
        "the incremental build wrote the sidecar"
    );
    repo
}

fn sidecar_path(repo: &Path) -> std::path::PathBuf {
    glia_store::default_gmap_dir(repo).join("parse_cache.bin")
}

fn split_u64(b: &[u8]) -> (u64, &[u8]) {
    let (n, rest) = b.split_first_chunk::<8>().expect("u64 field");
    (u64::from_le_bytes(*n), rest)
}

/// Every entry of the sidecar on disk as the bytes it holds for it, read
/// straight off the file: the CD.7a frame (`GLIAPCZ1`, stamp, raw length, lz4
/// block), then the bincode of the cache (stamp, identity, go.mod set, the
/// entry map), each entry's byte span cut out as it is walked.
fn sidecar_entries(repo: &Path) -> BTreeMap<String, Vec<u8>> {
    let bytes = std::fs::read(sidecar_path(repo)).expect("read sidecar");
    let rest = bytes.strip_prefix(b"GLIAPCZ1").expect("a framed sidecar");
    let (stamp_len, rest) = split_u64(rest);
    let (stamp, rest) = rest.split_at(usize::try_from(stamp_len).expect("stamp length"));
    assert_eq!(
        stamp,
        BUILD_STAMP.as_bytes(),
        "the sidecar was written by this build"
    );
    let (raw_len, body) = split_u64(rest);
    let raw = lz4_flex::block::decompress(body, usize::try_from(raw_len).expect("raw length"))
        .expect("lz4 body");
    let mut r: &[u8] = &raw;
    for field in ["stamp", "repo identity", "go.mod set"] {
        let _: String =
            bincode::deserialize_from(&mut r).unwrap_or_else(|e| panic!("{field}: {e}"));
    }
    let n: u64 = bincode::deserialize_from(&mut r).expect("entry count");
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let path: String = bincode::deserialize_from(&mut r).expect("entry path");
        let before = r;
        let _: (u64, String, FileParse) = bincode::deserialize_from(&mut r).expect("entry");
        out.insert(path, before[..before.len() - r.len()].to_vec());
    }
    assert!(r.is_empty(), "the entry map is the sidecar's last field");
    out
}

fn go_ctx() -> String {
    GoModules::from_entries(vec![(String::new(), "example.com/b".to_string())]).context_key()
}

#[test]
fn keys_move_with_every_input() {
    let base = (
        "stamp",
        "dir:repo",
        "python",
        "a.py",
        "a",
        "=example.com/b",
        b"x = 1\n".as_slice(),
    );
    let key = |f: (&str, &str, &str, &str, &str, &str, &[u8])| {
        file_key(f.0, f.1, f.2, f.3, f.4, f.5, f.6)
    };
    let k = key(base);
    let one_field_changed = [
        (
            "stamp",
            key(("stamp2", base.1, base.2, base.3, base.4, base.5, base.6)),
        ),
        (
            "repo key",
            key((base.0, "dir:other", base.2, base.3, base.4, base.5, base.6)),
        ),
        (
            "lang",
            key((base.0, base.1, "ruby", base.3, base.4, base.5, base.6)),
        ),
        (
            "path",
            key((base.0, base.1, base.2, "b.py", base.4, base.5, base.6)),
        ),
        (
            "module qname",
            key((base.0, base.1, base.2, base.3, "a.py", base.5, base.6)),
        ),
        (
            "content",
            key((base.0, base.1, base.2, base.3, base.4, base.5, b"x = 2\n")),
        ),
        // Length framing: the same bytes split differently between two fields.
        (
            "field boundary",
            key(("stam", "pdir:repo", base.2, base.3, base.4, base.5, base.6)),
        ),
    ];
    for (field, other) in one_field_changed {
        assert_ne!(k, other, "the key must move with the {field}");
    }
    // A non-Go file does not read the go.mod set, so its key ignores it ...
    assert_eq!(
        k,
        key((
            base.0,
            base.1,
            base.2,
            base.3,
            base.4,
            "=example.com/other",
            base.6
        )),
        "a python key moved with the go.mod set"
    );
    // ... and a Go file's key moves with it.
    let go = (
        "stamp",
        "dir:repo",
        "go",
        "b.go",
        "b",
        "=example.com/b",
        b"package main\n".as_slice(),
    );
    assert_ne!(
        key(go),
        key((go.0, go.1, go.2, go.3, go.4, "=example.com/other", go.6))
    );
    assert_ne!(key(go), key((go.0, go.1, go.2, go.3, go.4, "", go.6)));

    // Text form: 64 lower-case hex, round-trips, one spelling per key.
    let hex = k.to_hex();
    assert_eq!(hex.len(), 64);
    assert!(
        hex.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "{hex}"
    );
    assert_eq!(CacheKey::from_hex(&hex), Ok(k));
    assert_eq!(k.to_string(), hex);
    assert_eq!(CacheKey::from_bytes(*k.as_bytes()), k);
    assert_eq!(
        serde_json::to_string(&k).expect("serialize key"),
        format!("\"{hex}\"")
    );
    for bad in [
        hex.to_uppercase(),
        hex[..63].to_string(),
        format!("{hex}0"),
        format!(" {}", &hex[1..]),
        format!("g{}", &hex[1..]),
        String::new(),
    ] {
        assert!(CacheKey::from_hex(&bad).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn export_lists_every_valid_entry() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = built(tmp.path(), "repo");

    let rows = cache_rows(s(&repo)).expect("cache rows");
    assert_eq!(rows.stamp, BUILD_STAMP);
    assert_eq!(rows.repo_label, "repo");
    let planned: Vec<(&str, &str, &str)> = rows
        .rows
        .iter()
        .map(|r| (r.path.as_str(), r.lang, r.module_qname.as_str()))
        .collect();
    assert_eq!(
        planned, PARSED,
        "one row per language-parser file, path order"
    );

    // Each row's key is file_key over the build's own inputs.
    let ident = repo_identity(&repo).key;
    for r in &rows.rows {
        let content = std::fs::read(repo.join(&r.path)).expect("read source");
        let want = file_key(
            BUILD_STAMP,
            &ident,
            r.lang,
            &r.path,
            &r.module_qname,
            &go_ctx(),
            &content,
        );
        assert_eq!(r.key, want, "{}: key over other inputs", r.path);
        let text = std::str::from_utf8(&content).expect("utf-8 source");
        assert_eq!(
            r.content_hash,
            glia_engine::cache::content_hash(text),
            "{}",
            r.path
        );
    }

    let export = export_entries(s(&repo)).expect("export");
    assert_eq!((export.entries.len(), export.stale), (4, 0), "{export:?}");
    assert_eq!(export.stamp, BUILD_STAMP);
    assert_eq!(export.repo_label, "repo");

    let on_disk = sidecar_entries(&repo);
    assert_eq!(on_disk.len(), 4, "the sidecar holds the four parses");
    let key_of: BTreeMap<&str, CacheKey> =
        rows.rows.iter().map(|r| (r.path.as_str(), r.key)).collect();
    let exported: Vec<&str> = export.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        exported,
        ["a.py", "b.go", "util.js", "util.ts"],
        "entries in path order"
    );
    for e in &export.entries {
        assert_eq!(Some(&e.key), key_of.get(e.path.as_str()), "{}: key", e.path);
        assert_eq!(
            Some(&e.payload),
            on_disk.get(&e.path),
            "{}: the payload is not the sidecar entry's bytes",
            e.path
        );
    }
}

#[test]
fn stale_sidecar_exports_what_is_fresh() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = built(tmp.path(), "repo");
    let before = cache_rows(s(&repo)).expect("rows before the edit");

    std::fs::write(repo.join("a.py"), "def a():\n    return 2\n").expect("edit a.py");
    let export = export_entries(s(&repo)).expect("export");
    assert_eq!((export.entries.len(), export.stale), (3, 1), "{export:?}");
    let exported: Vec<&str> = export.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(exported, ["b.go", "util.js", "util.ts"]);

    // The edit moved a.py's key and no other.
    let after = cache_rows(s(&repo)).expect("rows after the edit");
    for (b, a) in before.rows.iter().zip(&after.rows) {
        assert_eq!(b.path, a.path);
        assert_eq!(
            b.key != a.key,
            a.path == "a.py",
            "{}: key moved wrongly",
            a.path
        );
    }
}

#[test]
fn another_identity_exports_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = built(tmp.path(), "repo");
    let before = cache_rows(s(&repo)).expect("rows under dir:repo");

    // The sidecar moves with the checkout, but `dir:moved` is another
    // identity: every cached parse carries `dir:repo` NodeIds.
    let moved = tmp.path().join("moved");
    std::fs::rename(&repo, &moved).expect("move the checkout");
    let export = export_entries(s(&moved)).expect("export");
    assert_eq!((export.entries.len(), export.stale), (0, 4), "{export:?}");

    let after = cache_rows(s(&moved)).expect("rows under dir:moved");
    assert_eq!(after.rows.len(), 4);
    for (b, a) in before.rows.iter().zip(&after.rows) {
        assert_eq!((&b.path, b.content_hash), (&a.path, a.content_hash));
        assert_ne!(b.key, a.key, "{}: the identity is not in the key", a.path);
    }
}

#[test]
fn another_go_module_set_exports_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = built(tmp.path(), "repo");
    let before = cache_rows(s(&repo)).expect("rows before");

    // A go.mod edit changes how every .go file parses: a build discards the
    // whole sidecar, so nothing of it is exported, a.py included.
    std::fs::write(repo.join("go.mod"), "module example.com/other\n\ngo 1.21\n")
        .expect("edit go.mod");
    let export = export_entries(s(&repo)).expect("export");
    assert_eq!((export.entries.len(), export.stale), (0, 4), "{export:?}");

    let after = cache_rows(s(&repo)).expect("rows after");
    for (b, a) in before.rows.iter().zip(&after.rows) {
        assert_eq!(
            b.key != a.key,
            a.lang == "go",
            "{}: only the Go key moves",
            a.path
        );
    }
}

#[test]
fn rows_are_deterministic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    write_repo(&repo);
    let a: CacheRows = cache_rows(s(&repo)).expect("rows a");
    let b: CacheRows = cache_rows(s(&repo)).expect("rows b");
    assert_eq!(a, b);
    assert_eq!(
        serde_json::to_string(&a).expect("json a"),
        serde_json::to_string(&b).expect("json b")
    );
    let paths: Vec<&str> = a.rows.iter().map(|r| r.path.as_str()).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted, "rows in path order");

    // No sidecar at all: every row is stale.
    let export = export_entries(s(&repo)).expect("export without a sidecar");
    assert_eq!((export.entries.len(), export.stale), (0, 4));
    assert!(cache_rows(s(&tmp.path().join("absent"))).is_err());
}
