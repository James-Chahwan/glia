//! CE.2a / CE.2b gate: the shared parse cache's engine half, keys, export and
//! the verified import.
//!
//! A cached parse is addressed by a blake3 key over every input the build's
//! language branch reads (build stamp, repo identity, language, path, MODULE
//! form, the go.mod set for a Go file, the content), and a checkout exports the
//! sidecar entries a build of it would reuse, each as the entry's own bytes.
//! Another checkout imports them (CE.2b) only for the files it lacks, only when
//! each payload is of the checked-out content, language and MODULE form, and
//! only when a re-parse of a random sample matches byte for byte.
//! The fixture repo is a.py, b.go + go.mod and the same-stem pair util.ts +
//! util.js (LB.13 names both MODULEs by file name), so the Go context and the
//! module form are both exercised. The import tests write one copy per side at
//! `<tmp>/<side>/repo`, so every copy's identity is `dir:repo`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glia_code_domain::FileParse;
use glia_code_domain::walk_gating::repo_identity;
use glia_core::NodeId;
use glia_engine::cache::CacheDiff;
use glia_engine::shared_cache::{
    CacheKey, CacheRows, ImportOptions, ImportSummary, Verify, cache_rows, export_entries,
    file_key, import_entries, wanted,
};
use glia_engine::{
    BUILD_STAMP, GoModules, ParseCache, generate_one, generate_one_incremental,
    generate_one_with_cache,
};
use glia_store::write_merged_sharded;

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

// ---- CE.2b: wanted rows and the verified import ----

fn opts(verify: Verify) -> ImportOptions {
    ImportOptions::default().with_verify(verify)
}

/// A fresh, unbuilt copy of the fixture at `<tmp>/<side>/repo`.
fn copy(tmp: &Path, side: &str) -> PathBuf {
    let repo = tmp.join(side).join("repo");
    write_repo(&repo);
    repo
}

/// Build side `a` (`<tmp>/a/repo`) and export its sidecar: `(path, key,
/// payload)` per entry, path order.
fn exported(tmp: &Path) -> (PathBuf, Vec<(String, CacheKey, Vec<u8>)>) {
    let a = built(&tmp.join("a"), "repo");
    let export = export_entries(s(&a)).expect("export a");
    assert_eq!((export.entries.len(), export.stale), (4, 0), "{export:?}");
    let entries = export
        .entries
        .into_iter()
        .map(|e| (e.path, e.key, e.payload))
        .collect();
    (a, entries)
}

fn pairs(entries: &[(String, CacheKey, Vec<u8>)]) -> Vec<(CacheKey, Vec<u8>)> {
    entries.iter().map(|(_, k, p)| (*k, p.clone())).collect()
}

fn entry<'a>(entries: &'a [(String, CacheKey, Vec<u8>)], path: &str) -> &'a (String, CacheKey, Vec<u8>) {
    entries
        .iter()
        .find(|(p, _, _)| p == path)
        .unwrap_or_else(|| panic!("{path} not exported"))
}

/// A payload's fields, decoded as the entry's derived shape.
fn unpack(payload: &[u8]) -> (u64, String, FileParse) {
    bincode::deserialize(payload).expect("decode payload")
}

fn pack(hash: u64, lang: &str, parse: &FileParse) -> Vec<u8> {
    bincode::serialize(&(hash, lang, parse)).expect("encode payload")
}

/// `(offered, accepted, rejected, verified, written)`.
fn counts(s: &ImportSummary) -> (usize, usize, usize, usize, bool) {
    (s.offered, s.accepted, s.rejected, s.verified, s.written)
}

fn wanted_paths(repo: &Path) -> (Vec<String>, usize) {
    let w = wanted(s(repo)).expect("wanted");
    assert_eq!((w.stamp, w.repo_label.as_str()), (BUILD_STAMP, "repo"));
    (w.rows.into_iter().map(|r| r.path).collect(), w.local_hits)
}

/// An incremental build of `repo` from its sidecar, as `generate_one_incremental`
/// runs it: the build's file diff, with the graph written to `out`.
fn warm_build(repo: &Path, out: &Path) -> CacheDiff {
    let mut cache = ParseCache::load(s(repo));
    let result = generate_one_with_cache(s(repo), &mut cache).expect("incremental build");
    write_merged_sharded(&result.merged, out).expect("write warm graph");
    cache.last_diff().expect("the build recorded a diff").clone()
}

fn cold_build(repo: &Path, out: &Path) {
    let result = generate_one(s(repo)).expect("cold build");
    write_merged_sharded(&result.merged, out).expect("write cold graph");
}

/// Every file of a written graph dir, by name.
fn dir_bytes(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .expect("read graph dir")
        .flatten()
        .map(|e| {
            let bytes = std::fs::read(e.path()).expect("read graph file");
            (e.file_name().to_string_lossy().into_owned(), bytes)
        })
        .collect()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn import_into_a_fresh_copy_reuses_every_parse() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (a, entries) = exported(tmp.path());
    let b = copy(tmp.path(), "b");

    // Same identity (`dir:repo`), same bytes: b wants exactly a's four keys.
    let w = wanted(s(&b)).expect("wanted");
    assert_eq!(w.local_hits, 0);
    let want: Vec<(&str, CacheKey)> = w.rows.iter().map(|r| (r.path.as_str(), r.key)).collect();
    let have: Vec<(&str, CacheKey)> = entries.iter().map(|(p, k, _)| (p.as_str(), *k)).collect();
    assert_eq!(want, have, "b's wanted rows are not a's exported keys");

    let summary = import_entries(s(&b), pairs(&entries), &opts(Verify::Count(0))).expect("import");
    assert_eq!(counts(&summary), (4, 4, 0, 0, true));
    assert_eq!(
        std::fs::read(sidecar_path(&b)).expect("b's sidecar"),
        std::fs::read(sidecar_path(&a)).expect("a's sidecar"),
        "the imported sidecar is not the one a's build wrote"
    );
    assert_eq!(wanted_paths(&b), (vec![], 4), "b still wants files");

    // The next incremental build reparses nothing and builds the cold graph.
    let diff = warm_build(&b, &tmp.path().join("warm"));
    assert_eq!(
        diff,
        CacheDiff {
            reused: strings(&["a.py", "b.go", "util.js", "util.ts"]),
            reparsed: vec![],
            evicted: vec![],
        }
    );
    cold_build(&b, &tmp.path().join("cold"));
    assert_eq!(
        dir_bytes(&tmp.path().join("warm")),
        dir_bytes(&tmp.path().join("cold")),
        "the graph built on imported parses differs from a cold build"
    );

    // Offered again, nothing is wanted: all rejected, the sidecar not rewritten.
    let again = import_entries(s(&b), pairs(&entries), &opts(Verify::All)).expect("re-import");
    assert_eq!(counts(&again), (4, 0, 4, 0, false));
}

#[test]
fn a_poisoned_payload_is_caught_by_the_sample() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (_a, mut entries) = exported(tmp.path());

    // util.ts's payload with one forged node: right key, content hash,
    // language and MODULE form, so only a re-parse can tell.
    let slot = entries
        .iter_mut()
        .find(|(p, _, _)| p == "util.ts")
        .expect("util.ts exported");
    let (hash, lang, mut parse) = unpack(&slot.2);
    let mut forged = parse.nodes.last().expect("util.ts has nodes").clone();
    forged.id = NodeId(forged.id.0 ^ 0x5eed);
    parse.nodes.push(forged);
    slot.2 = pack(hash, &lang, &parse);

    // The cheap checks pass it: with no sample it is written (the unsigned
    // store's documented limit; CE.2c's MAC is the defence).
    let c = copy(tmp.path(), "c");
    let trusted = import_entries(s(&c), pairs(&entries), &opts(Verify::Count(0))).expect("import");
    assert_eq!(counts(&trusted), (4, 4, 0, 0, true));

    // Re-parsing every entry catches it, and nothing is written.
    let b = copy(tmp.path(), "b");
    let err = import_entries(s(&b), pairs(&entries), &opts(Verify::All))
        .expect_err("a poisoned payload passed the full re-parse");
    assert!(err.contains("util.ts") && err.contains("differs from a local parse"), "{err}");
    assert!(!sidecar_path(&b).exists(), "a failed import wrote the sidecar");
    assert!(!glia_store::default_gmap_dir(&b).exists(), "a failed import created the layout dir");
}

#[test]
fn mismatched_payloads_are_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (a, entries) = exported(tmp.path());
    let (_, a_py_key, a_py) = entry(&entries, "a.py");
    let (_, b_go_key, b_go) = entry(&entries, "b.go");
    let (_, util_js_key, util_js) = entry(&entries, "util.js");
    let (_, util_ts_key, _) = entry(&entries, "util.ts");

    // a.py's parse with a stale content hash; b.go's relabelled as python.
    let (hash, lang, parse) = unpack(a_py);
    let stale = pack(hash ^ 1, &lang, &parse);
    let (hash, _, parse) = unpack(b_go);
    let relabelled = pack(hash, "python", &parse);

    let b = copy(tmp.path(), "b");
    let offered = vec![
        // util.js's payload under util.ts's key: another row's parse.
        (*util_ts_key, util_js.clone()),
        (*a_py_key, stale),
        (*b_go_key, relabelled),
        (*util_js_key, util_js.clone()),
    ];
    let summary = import_entries(s(&b), offered, &opts(Verify::All)).expect("import");
    assert_eq!(counts(&summary), (4, 1, 3, 1, true));
    assert_eq!(
        wanted_paths(&b),
        (strings(&["a.py", "b.go", "util.ts"]), 1),
        "only util.js was imported"
    );
    let on_disk = sidecar_entries(&b);
    assert_eq!(on_disk.keys().collect::<Vec<_>>(), ["util.js"]);
    assert_eq!(on_disk.get("util.js"), sidecar_entries(&a).get("util.js"));

    // What else a store can offer: a key this checkout never wants, bytes
    // that are no payload, a payload cut short or padded, a key offered twice.
    let c = copy(tmp.path(), "c");
    let mut cut = util_js.clone();
    cut.pop();
    let mut padded = util_js.clone();
    padded.push(0);
    let offered = vec![
        (CacheKey::from_bytes([7; 32]), util_js.clone()),
        (*a_py_key, b"not a payload".to_vec()),
        (*util_js_key, cut),
        (*util_js_key, padded),
        (*util_js_key, util_js.clone()),
        (*util_js_key, util_js.clone()),
    ];
    let summary = import_entries(s(&c), offered, &opts(Verify::All)).expect("import");
    assert_eq!(counts(&summary), (6, 1, 5, 1, true));
}

#[test]
fn module_form_change_misses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (_a, entries) = exported(tmp.path());

    // a.ts beside a.py: a cross-language stem, so a.py's MODULE turns from
    // `a` to the file-name form `a.py` (LB.9b) and its key moves; the other
    // three keep theirs.
    let b = copy(tmp.path(), "b");
    std::fs::write(b.join("a.ts"), "export const aTs = 1;\n").expect("write a.ts");
    let w = wanted(s(&b)).expect("wanted");
    let rows: Vec<(&str, &str)> = w
        .rows
        .iter()
        .map(|r| (r.path.as_str(), r.module_qname.as_str()))
        .collect();
    assert_eq!(
        rows,
        [("a.py", "a.py"), ("a.ts", "a.ts"), ("b.go", "b"), ("util.js", "util.js"), ("util.ts", "util.ts")]
    );
    let (_, a_py_key, a_py) = entry(&entries, "a.py");
    let b_a_py = w.rows.iter().find(|r| r.path == "a.py").expect("a.py row");
    assert_ne!(b_a_py.key, *a_py_key, "a.py's key ignored its MODULE form");

    // a's a.py payload: under a's key it names nothing b wants; under b's
    // key it is the other MODULE form.
    let mut offered = pairs(&entries);
    offered.push((b_a_py.key, a_py.clone()));
    let summary = import_entries(s(&b), offered, &opts(Verify::All)).expect("import");
    assert_eq!(counts(&summary), (5, 3, 2, 3, true));
    assert_eq!(wanted_paths(&b), (strings(&["a.py", "a.ts"]), 3));

    let diff = warm_build(&b, &tmp.path().join("warm"));
    assert_eq!(
        diff,
        CacheDiff {
            reused: strings(&["b.go", "util.js", "util.ts"]),
            reparsed: strings(&["a.py", "a.ts"]),
            evicted: vec![],
        }
    );
}

#[test]
fn parse_for_cache_matches_the_build() {
    // A cold incremental build wrote a's sidecar through the router; a full
    // re-parse of every file through `route::parse_for_cache` (the import's
    // verification) must give the very same entry bytes, file by file.
    let tmp = tempfile::tempdir().expect("tempdir");
    let (a, entries) = exported(tmp.path());
    let on_disk = sidecar_entries(&a);
    for (path, _, payload) in &entries {
        assert_eq!(Some(payload), on_disk.get(path), "{path}: export is not the sidecar entry");
    }
    let b = copy(tmp.path(), "b");
    let summary = import_entries(s(&b), pairs(&entries), &opts(Verify::All)).expect("verified import");
    assert_eq!(counts(&summary), (4, 4, 0, 4, true));
    assert_eq!(sidecar_entries(&b), on_disk);
}

#[test]
fn verify_sample_is_bounded() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (_a, entries) = exported(tmp.path());

    let sampled = copy(tmp.path(), "b");
    let summary =
        import_entries(s(&sampled), pairs(&entries), &opts(Verify::Count(2))).expect("sampled import");
    assert_eq!(counts(&summary), (4, 4, 0, 2, true));

    let all = copy(tmp.path(), "c");
    let summary = import_entries(s(&all), pairs(&entries), &opts(Verify::All)).expect("full import");
    assert_eq!(counts(&summary), (4, 4, 0, 4, true));

    // A sample larger than what was accepted checks what there is.
    let over = copy(tmp.path(), "d");
    let summary =
        import_entries(s(&over), pairs(&entries), &opts(Verify::Count(99))).expect("oversized sample");
    assert_eq!(counts(&summary), (4, 4, 0, 4, true));

    // The sample changes what is checked, never what is written.
    let sidecar = |repo: &Path| std::fs::read(sidecar_path(repo)).expect("read sidecar");
    assert_eq!(sidecar(&sampled), sidecar(&all));
    assert_eq!(sidecar(&over), sidecar(&all));
}

#[test]
fn import_follows_the_checkout_not_the_offer() {
    // A file edited after `wanted` ran: its key moved, so the payload fetched
    // for the old bytes names nothing, and the rest still import.
    let tmp = tempfile::tempdir().expect("tempdir");
    let (_a, entries) = exported(tmp.path());
    let b = copy(tmp.path(), "b");
    assert_eq!(wanted(s(&b)).expect("wanted").rows.len(), 4);
    std::fs::write(b.join("a.py"), "def a():\n    return 3\n").expect("edit a.py");
    let summary = import_entries(s(&b), pairs(&entries), &opts(Verify::All)).expect("import");
    assert_eq!(counts(&summary), (4, 3, 1, 3, true));
    assert_eq!(wanted_paths(&b), (strings(&["a.py"]), 3));

    // Nothing accepted writes nothing: no layout dir appears.
    let c = copy(tmp.path(), "c");
    let summary = import_entries(s(&c), vec![], &opts(Verify::All)).expect("empty import");
    assert_eq!(counts(&summary), (0, 0, 0, 0, false));
    assert!(!glia_store::default_gmap_dir(&c).exists());
    assert!(import_entries(s(&tmp.path().join("absent")), vec![], &opts(Verify::All)).is_err());
}
