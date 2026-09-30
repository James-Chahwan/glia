//! CE.2c — `glia cache push|pull|gc`, the shared parse cache's CLI transport,
//! driven through the real binary over a directory store.
//!
//! The fixture repo is the engine's shared-cache one (engine/tests/shared_cache.rs):
//! a.py, b.go + go.mod and the same-stem pair util.ts + util.js, four files a
//! build hands a language parser. Each side is a copy at `<tmp>/<side>/repo`,
//! so every copy's repo identity is `dir:repo` and the sides share keys.
//! The fired_on markers are grep-able:
//! `cargo test -p glia-cli --test cache_cli -- --nocapture 2>&1 | grep -o '\[cache\] .*'`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_engine::BUILD_STAMP;
use glia_engine::shared_cache::{cache_rows, export_entries};

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

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OTHER_KEY: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

/// A scratch dir, created fresh and removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`).
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-ce2c-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    /// A fresh copy of the fixture at `<scratch>/<side>/repo`.
    fn repo(&self, side: &str) -> PathBuf {
        let repo = self.0.join(side).join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir");
        for (name, text) in FILES {
            std::fs::write(repo.join(name), text).expect("write fixture file");
        }
        repo
    }

    /// A key file holding `hex`, with `mode` on unix.
    fn key_file(&self, name: &str, hex: &str, mode: u32) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, format!("{hex}\n")).expect("write key file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .expect("chmod key file");
        }
        #[cfg(not(unix))]
        let _ = mode;
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 temp path")
}

/// Run the real binary with no cache key or token in its environment and
/// `GLIA_NO_PERSIST=1`, plus `envs`. Its `[cache]` lines are echoed.
fn glia(args: &[&str], envs: &[(&str, &str)]) -> Run {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
    cmd.args(args)
        .env_remove("GLIA_CACHE_KEY")
        .env_remove("GLIA_CACHE_TOKEN")
        .env("GLIA_NO_PERSIST", "1");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run glia");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    for line in stderr.lines().filter(|l| l.starts_with("[cache] ")) {
        eprintln!("{line}");
    }
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr,
    }
}

/// `glia build <repo>`, persisting (`GLIA_NO_PERSIST` unset): writes the
/// layout and the parse-cache sidecar.
fn build(repo: &Path) -> Run {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
    cmd.args(["build", s(repo)])
        .env_remove("GLIA_NO_PERSIST")
        .env_remove("GLIA_CACHE_KEY")
        .env_remove("GLIA_CACHE_TOKEN");
    let out = cmd.output().expect("run glia build");
    let run = Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    };
    assert_eq!(
        run.code,
        0,
        "glia build {}:\n{}",
        repo.display(),
        run.stderr
    );
    run
}

/// The `key=value` fields of the first stderr line starting with `prefix`.
fn marker(run: &Run, prefix: &str) -> BTreeMap<String, String> {
    let line = run
        .stderr
        .lines()
        .find(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` line in:\n{}", run.stderr));
    line[prefix.len()..]
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Assert `fields` of a marker.
fn assert_fields(m: &BTreeMap<String, String>, want: &[(&str, &str)], run: &Run) {
    for (k, v) in want {
        assert_eq!(
            m.get(*k).map(String::as_str),
            Some(*v),
            "{k} in {m:?}\nstderr:\n{}",
            run.stderr
        );
    }
}

fn sidecar(repo: &Path) -> PathBuf {
    repo.join(".glia/graph/parse_cache.bin")
}

/// Every object file under `<store>/v1/<stamp>/`, sorted.
fn objects(store: &Path, stamp: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(fans) = std::fs::read_dir(store.join("v1").join(stamp)) else {
        return out;
    };
    for fan in fans.flatten().filter(|e| e.path().is_dir()) {
        for f in std::fs::read_dir(fan.path())
            .expect("fan-out dir")
            .flatten()
        {
            out.push(f.path());
        }
    }
    out.sort();
    out
}

#[test]
fn dir_store_round_trip() {
    let t = Scratch::new("round");
    let (a, b) = (t.repo("a"), t.repo("b"));
    let store = t.0.join("store");
    let key = t.key_file("k.hex", KEY, 0o600);
    build(&a);

    let push = glia(
        &["cache", "push", s(&a), s(&store), "--key-file", s(&key)],
        &[],
    );
    assert_eq!(push.code, 0, "{}", push.stderr);
    assert_fields(
        &marker(&push, "[cache] export "),
        &[("repo", "repo"), ("entries", "4"), ("stale", "0")],
        &push,
    );
    let m = marker(&push, "[cache] push ");
    assert_fields(
        &m,
        &[
            ("store", s(&store)),
            ("repo", "repo"),
            ("entries", "4"),
            ("uploaded", "4"),
            ("present", "0"),
            ("stale", "0"),
            ("signed", "yes"),
        ],
        &push,
    );
    assert!(
        !push.stderr.contains("readable by other users"),
        "{}",
        push.stderr
    );
    assert!(
        !push.stderr.contains(KEY) && !push.stdout.contains(KEY),
        "the key was echoed"
    );
    // The layout: v1/<stamp>/<aa>/<key>.gpc, one per entry, plus LAST.
    let objs = objects(&store, BUILD_STAMP);
    assert_eq!(objs.len(), 4, "{objs:?}");
    for o in &objs {
        let name = o.file_name().and_then(|n| n.to_str()).expect("object name");
        let fan = o
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .expect("fan");
        assert!(
            name.ends_with(".gpc") && name.len() == 64 + 4 && name.starts_with(fan),
            "{o:?}"
        );
    }
    assert!(store.join("v1").join(BUILD_STAMP).join("LAST").is_file());

    let pull = glia(
        &["cache", "pull", s(&b), s(&store), "--key-file", s(&key)],
        &[],
    );
    assert_eq!(pull.code, 0, "{}", pull.stderr);
    assert_fields(
        &marker(&pull, "[cache] import "),
        &[
            ("repo", "repo"),
            ("offered", "4"),
            ("accepted", "4"),
            ("rejected", "0"),
            ("verified", "0"),
        ],
        &pull,
    );
    assert_fields(
        &marker(&pull, "[cache] pull "),
        &[
            ("repo", "repo"),
            ("files", "4"),
            ("local_hits", "0"),
            ("fetched", "4"),
            ("missing", "0"),
            ("rejected", "0"),
            ("verified", "0"),
            ("signed", "yes"),
        ],
        &pull,
    );
    assert!(sidecar(&b).is_file(), "the pull wrote no sidecar");

    // The next build of b reparses nothing.
    let warm = build(&b);
    assert!(
        warm.stderr.contains("reused 4, reparsed 0, evicted 0"),
        "{}",
        warm.stderr
    );

    // A second push finds every object present; a second pull wants nothing.
    let again = glia(
        &[
            "cache",
            "push",
            s(&a),
            s(&store),
            "--key-file",
            s(&key),
            "--json",
        ],
        &[],
    );
    assert_eq!(again.code, 0, "{}", again.stderr);
    assert_fields(
        &marker(&again, "[cache] push "),
        &[("uploaded", "0"), ("present", "4")],
        &again,
    );
    let json: serde_json::Value = serde_json::from_str(again.stdout.trim()).expect("push --json");
    assert_eq!(
        (json["uploaded"].as_u64(), json["present"].as_u64()),
        (Some(0), Some(4))
    );
    assert_eq!(json["objects"].as_array().map(Vec::len), Some(4));
    let pull2 = glia(
        &["cache", "pull", s(&b), s(&store), "--key-file", s(&key)],
        &[],
    );
    assert_eq!(pull2.code, 0, "{}", pull2.stderr);
    assert_fields(
        &marker(&pull2, "[cache] pull "),
        &[
            ("files", "4"),
            ("local_hits", "4"),
            ("fetched", "0"),
            ("missing", "0"),
        ],
        &pull2,
    );
}

#[test]
fn keyed_pull_rejects_foreign_objects() {
    let t = Scratch::new("foreign");
    let (a, b) = (t.repo("a"), t.repo("b"));
    let store = t.0.join("store");
    let k = t.key_file("k.hex", KEY, 0o600);
    let k2 = t.key_file("k2.hex", OTHER_KEY, 0o644);
    build(&a);
    let push = glia(
        &["cache", "push", s(&a), s(&store), "--key-file", s(&k)],
        &[],
    );
    assert_eq!(push.code, 0, "{}", push.stderr);

    let pull = glia(
        &["cache", "pull", s(&b), s(&store), "--key-file", s(&k2)],
        &[],
    );
    assert_eq!(pull.code, 0, "{}", pull.stderr);
    #[cfg(unix)]
    assert!(
        pull.stderr.contains("readable by other users"),
        "{}",
        pull.stderr
    );
    assert_fields(
        &marker(&pull, "[cache] pull "),
        &[
            ("fetched", "0"),
            ("rejected", "4"),
            ("missing", "0"),
            ("signed", "yes"),
        ],
        &pull,
    );
    let rejections = pull
        .stderr
        .lines()
        .filter(|l| l.starts_with("[cache] rejected v1/") && l.contains("MAC does not verify"))
        .count();
    assert_eq!(rejections, 4, "{}", pull.stderr);
    assert!(
        !pull.stderr.contains(OTHER_KEY) && !pull.stdout.contains(OTHER_KEY),
        "the key was echoed"
    );
    assert!(!sidecar(&b).exists(), "a rejected pull wrote b's sidecar");
}

#[test]
fn poisoned_unsigned_store_is_caught() {
    let t = Scratch::new("poison");
    let (a, b) = (t.repo("a"), t.repo("b"));
    // p: a.py with one extra function, so its parse has one extra node.
    let p = t.repo("p");
    std::fs::write(
        p.join("a.py"),
        "def a():\n    return 1\n\n\ndef extra():\n    return 2\n",
    )
    .expect("write p/a.py");
    let store = t.0.join("store");
    build(&a);
    build(&p);
    let push = glia(&["cache", "push", s(&a), s(&store), "--unsigned"], &[]);
    assert_eq!(push.code, 0, "{}", push.stderr);
    assert_fields(
        &marker(&push, "[cache] push "),
        &[("uploaded", "4"), ("signed", "no")],
        &push,
    );

    // Rewrite a.py's object as a valid unsigned object whose payload is p's
    // parse of a.py relabelled with b's content hash (the payload's first
    // field): right key, content hash, language and MODULE form, so only a
    // re-parse tells it apart.
    let row = cache_rows(s(&b))
        .expect("b's rows")
        .rows
        .into_iter()
        .find(|r| r.path == "a.py")
        .expect("a.py row");
    let mut payload = export_entries(s(&p))
        .expect("export p")
        .entries
        .into_iter()
        .find(|e| e.path == "a.py")
        .expect("p exports a.py")
        .payload;
    payload[..8].copy_from_slice(&row.content_hash.to_le_bytes());
    let mut object = b"GLIAPC01".to_vec();
    object.push(0);
    object.extend_from_slice(row.key.as_bytes());
    object.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    object.extend_from_slice(&payload);
    let hex = row.key.to_hex();
    let rel = format!("v1/{BUILD_STAMP}/{}/{hex}.gpc", &hex[..2]);
    assert!(store.join(&rel).is_file(), "a.py's object is not at {rel}");
    std::fs::write(store.join(&rel), &object).expect("poison a.py's object");

    let pull = glia(
        &[
            "cache",
            "pull",
            s(&b),
            s(&store),
            "--unsigned",
            "--verify",
            "all",
        ],
        &[],
    );
    assert_eq!(pull.code, 1, "{}", pull.stderr);
    assert!(
        pull.stderr.contains("a.py") && pull.stderr.contains("differs from a local parse"),
        "{}",
        pull.stderr
    );
    assert!(!sidecar(&b).exists(), "a poisoned pull wrote b's sidecar");
}

#[test]
fn no_key_needs_unsigned() {
    let t = Scratch::new("nokey");
    let (a, b, c) = (t.repo("a"), t.repo("b"), t.repo("c"));
    let store = t.0.join("store");
    std::fs::create_dir_all(&store).expect("store dir");

    let refused = glia(&["cache", "pull", s(&b), s(&store)], &[]);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    assert!(
        refused.stderr.contains(
            "no cache key: set GLIA_CACHE_KEY or pass --key-file (or --unsigned to trust the store as-is)"
        ),
        "{}",
        refused.stderr
    );
    let refused = glia(&["cache", "push", s(&a), s(&store)], &[]);
    assert_eq!(refused.code, 2, "{}", refused.stderr);

    build(&a);
    let push = glia(&["cache", "push", s(&a), s(&store), "--unsigned"], &[]);
    assert_eq!(push.code, 0, "{}", push.stderr);
    let pull = glia(
        &["cache", "pull", s(&b), s(&store), "--unsigned", "--json"],
        &[],
    );
    assert_eq!(pull.code, 0, "{}", pull.stderr);
    let json: serde_json::Value = serde_json::from_str(pull.stdout.trim()).expect("pull --json");
    assert_eq!(json["verify"], 32, "{json}");
    assert_eq!(json["signed"], false, "{json}");
    assert_eq!(json["import"]["verified"], 4, "{json}");
    assert_fields(
        &marker(&pull, "[cache] pull "),
        &[("fetched", "4"), ("verified", "4"), ("signed", "no")],
        &pull,
    );
    assert!(sidecar(&b).is_file());

    // A key in the environment wins over --unsigned: the store's unsigned
    // objects are refused.
    let keyed = glia(
        &["cache", "pull", s(&c), s(&store), "--unsigned"],
        &[("GLIA_CACHE_KEY", KEY)],
    );
    assert_eq!(keyed.code, 0, "{}", keyed.stderr);
    assert!(
        keyed.stderr.contains("note: a cache key is set"),
        "{}",
        keyed.stderr
    );
    assert_fields(
        &marker(&keyed, "[cache] pull "),
        &[("fetched", "0"), ("rejected", "4"), ("signed", "yes")],
        &keyed,
    );
    assert!(!keyed.stderr.contains(KEY), "the key was echoed");
    assert!(!sidecar(&c).exists());

    // A malformed key is a usage error that does not quote it.
    let bad = glia(
        &["cache", "pull", s(&c), s(&store)],
        &[("GLIA_CACHE_KEY", "not-a-key-at-all")],
    );
    assert_eq!(bad.code, 2, "{}", bad.stderr);
    assert!(!bad.stderr.contains("not-a-key-at-all"), "{}", bad.stderr);
}

#[test]
fn gc_prunes() {
    let t = Scratch::new("gc");
    let a = t.repo("a");
    let store = t.0.join("store");
    let key = t.key_file("k.hex", KEY, 0o600);
    build(&a);
    let push = glia(
        &["cache", "push", s(&a), s(&store), "--key-file", s(&key)],
        &[],
    );
    assert_eq!(push.code, 0, "{}", push.stderr);

    // An older release's stamp: one object, LAST ten days ago.
    let old = store.join("v1").join("0.0.0+p0000000000000000");
    std::fs::create_dir_all(old.join("ab")).expect("old stamp");
    std::fs::write(
        old.join("ab").join(format!("ab{}.gpc", "0".repeat(62))),
        b"old object",
    )
    .expect("old object");
    let last = old.join("LAST");
    std::fs::write(&last, b"").expect("old LAST");
    let ten_days = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 24 * 3600);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&last)
        .and_then(|f| f.set_modified(ten_days))
        .expect("age LAST");

    let gc = glia(&["cache", "gc", s(&store), "--keep-stamps", "1"], &[]);
    assert_eq!(gc.code, 0, "{}", gc.stderr);
    assert_fields(
        &marker(&gc, "[cache] gc "),
        &[
            ("stamps_kept", "1"),
            ("stamps_removed", "1"),
            ("objects_removed", "1"),
        ],
        &gc,
    );
    assert!(!old.exists(), "the old stamp survived gc");
    assert_eq!(
        objects(&store, BUILD_STAMP).len(),
        4,
        "gc touched the current stamp"
    );

    let missing = glia(&["cache", "gc", s(&t.0.join("nope"))], &[]);
    assert_eq!(missing.code, 2, "{}", missing.stderr);
}

#[test]
fn a_url_store_is_refused_until_ce_2e() {
    let t = Scratch::new("url");
    let repo = t.repo("a");
    let run = glia(
        &[
            "cache",
            "pull",
            s(&repo),
            "https://example.com/x",
            "--unsigned",
        ],
        &[],
    );
    assert_eq!(run.code, 2, "{}", run.stderr);
    assert!(run.stderr.contains("unsupported store"), "{}", run.stderr);
    assert!(
        !run.stderr.contains("[cache] pull "),
        "a URL store ran a pull"
    );
}
