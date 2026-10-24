//! FB.5b — `glia embed`, the CLI surface of the embedding snapshot step
//! (`glia_engine::semantic::embed_repo`, FB.5a), driving the real binary over
//! scratch repos. Offline: every run uses `--model test-hash` except the cache
//! refusal, which must exit before the bge-small model could load.
//!
//! fired_on, on stderr: the engine's `[embed] repo=<label> model=<m> recipe=<t>
//! nodes=.. embedded=<e> ..`, then `[embed] cli repo=<label> exit=<code>`.
//! Grep them with
//! `cargo test -p glia-cli --test embed_cli -- --nocapture 2>&1 | grep '^\[embed\] '`.

use std::path::PathBuf;
use std::process::{Command, Output};

const APP: &str = "def fetch_invoice(invoice_id):\n    \"\"\"Fetch one invoice.\"\"\"\n    return invoice_id\n\n\n\
                   def draw_chart(points):\n    \"\"\"Draw the chart.\"\"\"\n    return list(points)\n";

/// A scratch repo holding `app.py`, removed on drop (`cli` has no `tempfile`).
struct Scratch {
    root: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir()
            .join(format!("glia-fb5b-{}-{name}", std::process::id()))
            .join("repo");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch dir");
        std::fs::write(root.join("app.py"), APP).expect("write app.py");
        Scratch { root }
    }

    fn path(&self) -> &str {
        self.root.to_str().expect("utf-8 scratch path")
    }

    fn vectors(&self) -> PathBuf {
        self.root.join(".glia").join("vectors.jsonl")
    }

    /// `glia embed <repo> <args..>`, in memory (GLIA_NO_PERSIST) unless
    /// `persist`.
    fn embed(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
        cmd.arg("embed").arg(self.path()).args(args);
        cmd.env_remove("GLIA_EMBED_MODEL");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().expect("run glia")
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Removes the scratch's parent dir too.
struct Guard(PathBuf);

fn guard(s: &Scratch) -> Guard {
    Guard(s.root.parent().expect("parent").to_path_buf())
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

const NO_PERSIST: (&str, &str) = ("GLIA_NO_PERSIST", "1");

#[test]
fn embed_twice_embeds_nothing_the_second_time() {
    let s = Scratch::new("twice");
    let _g = guard(&s);
    let first = s.embed(&["--model", "test-hash"], &[]);
    let (out, err) = (text(&first.stdout), text(&first.stderr));
    assert_eq!(
        first.status.code(),
        Some(0),
        "stdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        out.starts_with("embedded ") && !out.starts_with("embedded 0,"),
        "{out}"
    );
    assert!(
        out.contains("-> .glia/vectors.jsonl (")
            && out.contains("model glia-test-hash-v1/64, recipe t1)"),
        "{out}"
    );
    assert!(
        err.contains("[embed] repo=repo model=glia-test-hash-v1/64 recipe=t1 nodes="),
        "{err}"
    );
    assert!(err.contains("[embed] cli repo=repo exit=0"), "{err}");
    let body = std::fs::read_to_string(s.vectors()).expect("vectors.jsonl written");
    assert!(body.contains("fetch_invoice"), "{body}");

    let second = s.embed(&["--model", "test-hash"], &[]);
    let out2 = text(&second.stdout);
    assert_eq!(second.status.code(), Some(0), "{}", text(&second.stderr));
    assert!(out2.starts_with("embedded 0, cached "), "{out2}");
    assert!(
        text(&second.stderr).contains(" embedded=0 "),
        "{}",
        text(&second.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(s.vectors()).expect("vectors.jsonl"),
        body,
        "an unchanged tree rewrites the same bytes"
    );
}

#[test]
fn check_exits_one_after_an_edit_and_writes_nothing() {
    let s = Scratch::new("check");
    let _g = guard(&s);
    let missing = s.embed(&["--model", "test-hash", "--check"], &[NO_PERSIST]);
    assert_eq!(missing.status.code(), Some(1), "{}", text(&missing.stderr));
    assert!(!s.vectors().exists(), "--check writes nothing");
    assert!(
        text(&missing.stdout)
            .contains(" nodes need embedding (model glia-test-hash-v1/64, recipe t1)"),
        "{}",
        text(&missing.stdout)
    );

    let run = s.embed(&["--model", "test-hash"], &[NO_PERSIST]);
    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    let body = std::fs::read_to_string(s.vectors()).expect("vectors.jsonl");

    let fresh = s.embed(&["--model", "test-hash", "--check"], &[NO_PERSIST]);
    assert_eq!(fresh.status.code(), Some(0), "{}", text(&fresh.stderr));
    assert_eq!(
        text(&fresh.stdout).trim(),
        "0 nodes need embedding (model glia-test-hash-v1/64, recipe t1)"
    );

    let edited = APP.replace("Fetch one invoice.", "Fetch one unpaid invoice by id.");
    std::fs::write(s.root.join("app.py"), edited).expect("edit app.py");
    let stale = s.embed(&["--model", "test-hash", "--check"], &[NO_PERSIST]);
    let err = text(&stale.stderr);
    assert_eq!(stale.status.code(), Some(1), "{err}");
    assert_eq!(
        text(&stale.stdout).trim(),
        "1 nodes need embedding (model glia-test-hash-v1/64, recipe t1)"
    );
    assert!(err.contains("[embed] cli repo=repo exit=1"), "{err}");
    assert_eq!(
        std::fs::read_to_string(s.vectors()).expect("vectors.jsonl"),
        body,
        "--check leaves the sidecar alone"
    );

    // --json under --check: the report, still exit 1.
    let json = s.embed(
        &["--model", "test-hash", "--check", "--json"],
        &[NO_PERSIST],
    );
    assert_eq!(json.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&json.stdout).expect("--json is JSON");
    assert_eq!(v["recipe"], "t1");
    assert_eq!(v["model"], "glia-test-hash-v1/64");
    assert_eq!(v["embedded"], 1);
    assert_eq!(v["rows_written"], 0);
}

#[test]
fn unknown_model_exits_two_naming_the_models() {
    let s = Scratch::new("nope");
    let _g = guard(&s);
    let out = s.embed(&["--model", "nope"], &[NO_PERSIST]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(
        err.contains("bge-small") && err.contains("test-hash"),
        "{err}"
    );
    assert!(!s.vectors().exists());
}

#[test]
fn another_models_sidecar_needs_replace() {
    let s = Scratch::new("replace");
    let _g = guard(&s);
    std::fs::create_dir_all(s.root.join(".glia")).expect(".glia");
    let foreign = "{\"qname\":\"app::fetch_invoice\",\"kind\":\"FUNCTION\",\"model\":\"other-model\",\"dims\":2,\"b64\":\"AACAPwAAAAA=\"}\n";
    std::fs::write(s.vectors(), foreign).expect("write a foreign sidecar");

    let refused = s.embed(&["--model", "test-hash"], &[NO_PERSIST]);
    let err = text(&refused.stderr);
    assert_eq!(refused.status.code(), Some(2), "{err}");
    assert!(
        err.contains("other-model") && err.contains("--replace"),
        "{err}"
    );
    assert!(err.contains("[embed] cli repo=repo exit=2"), "{err}");
    assert_eq!(
        std::fs::read_to_string(s.vectors()).expect("sidecar"),
        foreign,
        "a refused run writes nothing"
    );

    let replaced = s.embed(
        &["--model", "test-hash", "--replace", "--json"],
        &[NO_PERSIST],
    );
    assert_eq!(
        replaced.status.code(),
        Some(0),
        "{}",
        text(&replaced.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&replaced.stdout).expect("JSON");
    assert_eq!(v["stale_dropped"], 1);
    let body = std::fs::read_to_string(s.vectors()).expect("sidecar");
    assert!(!body.contains("other-model"), "{body}");
}

#[test]
fn limit_defers_the_rest() {
    let s = Scratch::new("limit");
    let _g = guard(&s);
    let out = s.embed(
        &["--model", "test-hash", "--limit", "1", "--json"],
        &[NO_PERSIST],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(v["embedded"], 1);
    assert!(v["deferred"].as_u64().is_some_and(|d| d >= 1), "{v}");
}

#[test]
fn bge_small_refuses_a_cache_inside_the_repo_before_loading() {
    let s = Scratch::new("cache");
    let _g = guard(&s);
    let cache = s.root.join("x");
    let cache_s = cache.to_str().expect("utf-8").to_string();
    let out = s.embed(
        &["--model", "bge-small"],
        &[NO_PERSIST, ("FASTEMBED_CACHE_DIR", cache_s.as_str())],
    );
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("is inside the repository"), "{err}");
    assert!(
        !err.contains("[embed] model load"),
        "no model loaded: {err}"
    );
    assert!(!cache.exists(), "nothing written into the repo: {err}");
    assert!(!s.vectors().exists());
    assert!(err.contains("[embed] cli repo=repo exit=2"), "{err}");
}
