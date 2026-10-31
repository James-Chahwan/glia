//! DC.8 — `glia watch <repo>`, the foreground loop over the engine's
//! `WatchSession` (DC.7a-c), driving the real binary over scratch repos.
//!
//! Every read of the child's stdout goes through a reader thread and an mpsc
//! channel with a 30 s timeout, and every wait for its exit is bounded the
//! same way, so a hung watch fails the test instead of hanging it. The
//! child's stderr goes to a file in the scratch dir (never a pipe nobody
//! drains), read once the child is gone.
//!
//! The fired_on markers are this surface's `[watch] listening ...` (GA.11:
//! `max_latency_ms=<m>`, and `repos=<n>` under `--with`), `[watch] emitted
//! ...`, `[watch] persisted seq=<s> reason=interval|exit [out=<dir>]` and
//! `[watch] exit reason=...` lines; asserting them here makes them a tested
//! contract. GA.11's tests: `--with` / `--out` over two scratch repos, the
//! `--out` requirement, and the `--max-latency-ms` cap applying a path that
//! never stops moving. Grep them from a run of the binary:
//! `glia watch <repo> 2>&1 >/dev/null </dev/null | grep '^\[watch\] '`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(30);

const A_PY: &str = "def foo():\n    return bar()\n\n\ndef bar():\n    return 1\n";
/// `foo`'s body edited; a different length, so the stat alone shows it.
const A_PY_EDITED: &str = "def foo():\n    return bar() + 22\n\n\ndef bar():\n    return 1\n";

const B_PY: &str = "def b():\n    return 2\n";
/// `b`'s body edited, a different length.
const B_PY_EDITED: &str = "def b():\n    return 2 + 333\n";

/// A scratch dir holding a repo (`repo/`, one Python file); removed on drop
/// (`cli` has no dev-dependencies, so no `tempfile`). Canonical, so the
/// paths the binary prints compare equal.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-dc8-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo")).expect("scratch repo dir");
        let dir = dir.canonicalize().expect("canonical scratch dir");
        std::fs::write(dir.join("repo/a.py"), A_PY).expect("write a.py");
        Scratch(dir)
    }

    fn repo(&self) -> PathBuf {
        self.0.join("repo")
    }

    fn repo_str(&self) -> String {
        self.repo()
            .to_str()
            .expect("utf-8 scratch path")
            .to_string()
    }

    /// GA.11: a second repo beside the first, `repo2/` with one Python
    /// file; its path as the CLI is handed it.
    fn second(&self) -> String {
        std::fs::create_dir_all(self.0.join("repo2")).expect("second repo dir");
        std::fs::write(self.0.join("repo2/b.py"), B_PY).expect("write b.py");
        self.0
            .join("repo2")
            .to_str()
            .expect("utf-8 scratch path")
            .to_string()
    }

    fn stderr_path(&self) -> PathBuf {
        self.0.join("watch.stderr")
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(self.stderr_path()).unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A running `glia watch`: the child, its stdin (dropping it is the EOF) and
/// its stdout lines, read on a thread.
struct Watch {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl Watch {
    fn spawn(s: &Scratch, args: &[&str]) -> Watch {
        let stderr = std::fs::File::create(s.stderr_path()).expect("stderr file");
        let mut child = Command::new(env!("CARGO_BIN_EXE_glia"))
            .arg("watch")
            .arg(s.repo())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn glia watch");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Watch {
            child,
            stdin,
            lines: rx,
        }
    }

    /// The next stdout line as JSON, within [`TIMEOUT`].
    fn next_json(&mut self, s: &Scratch, what: &str) -> Value {
        match self.lines.recv_timeout(TIMEOUT) {
            Ok(line) => serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {line}")),
            Err(e) => {
                let _ = self.child.kill();
                panic!("{what}: no stdout line ({e}); stderr:\n{}", s.stderr())
            }
        }
    }

    /// Wait for the exit within [`TIMEOUT`]: its code.
    fn wait(&mut self, s: &Scratch) -> i32 {
        let started = Instant::now();
        loop {
            match self.child.try_wait().expect("try_wait") {
                Some(status) => return status.code().unwrap_or(-1),
                None if started.elapsed() > TIMEOUT => {
                    let _ = self.child.kill();
                    panic!("glia watch did not exit; stderr:\n{}", s.stderr());
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every file of a layout dir by name, but the parse cache and the
/// self-ignore.
fn layout_bytes(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let e = e.expect("dir entry");
        let name = e.file_name().to_string_lossy().into_owned();
        if name == "parse_cache.bin" || name == ".gitignore" {
            continue;
        }
        out.insert(name, std::fs::read(e.path()).expect("read a layout file"));
    }
    out
}

fn ready(w: &mut Watch, s: &Scratch) -> Value {
    let r = w.next_json(s, "ready line");
    assert_eq!(r["ready"], Value::Bool(true), "{r}");
    assert_eq!(r["seq"], 0, "{r}");
    assert_eq!(r["repo"].as_str(), Some(s.repo_str().as_str()), "{r}");
    assert!(r["total_nodes"].as_u64().is_some_and(|n| n > 0), "{r}");
    assert!(r["total_edges"].as_u64().is_some(), "{r}");
    r
}

#[test]
fn streams_an_update_and_persists_on_eof() {
    let s = Scratch::new("eof");
    let mut w = Watch::spawn(
        &s,
        &[
            "--poll-ms",
            "20",
            "--debounce-ms",
            "20",
            "--persist-every",
            "0",
        ],
    );
    ready(&mut w, &s);
    // Written beside the repo and renamed in, so no scan can see a.py
    // truncated and half-written (one update, never two).
    let staged = s.0.join("a.py.new");
    std::fs::write(&staged, A_PY_EDITED).expect("stage the edit");
    std::fs::rename(&staged, s.repo().join("a.py")).expect("edit a.py");
    let u = w.next_json(&s, "the update");
    assert_eq!(u["seq"], 1, "{u}");
    assert_eq!(u["files"]["modified"], serde_json::json!(["a.py"]), "{u}");
    for key in [
        "seq",
        "files",
        "groups_rebuilt",
        "body_only",
        "full_rebuild",
        "total_nodes",
        "total_edges",
        "counts",
        "nodes",
        "edges",
        "phase_ms",
    ] {
        assert!(u.get(key).is_some(), "update has `{key}`: {u}");
    }
    // GA.4: a body-only edit of a.py is re-stitched by the fast path, and
    // `body_only` (an array, a subset of groups_rebuilt) says so.
    assert_eq!(u["groups_rebuilt"], serde_json::json!(["python"]), "{u}");
    assert_eq!(u["body_only"], serde_json::json!(["python"]), "{u}");
    let nodes = u["nodes"].as_array().expect("nodes rows");
    assert!(
        nodes
            .iter()
            .any(|n| n["change"] == "modified" && n["qname"] == "a::foo"),
        "a modified row for foo: {u}"
    );
    // Not persisted yet: --persist-every 0 writes only on exit.
    let layout = s.repo().join(".glia/graph");
    assert!(!layout.join("manifest.json").exists());
    drop(w.stdin.take());
    assert_eq!(w.wait(&s), 0, "stderr:\n{}", s.stderr());
    let err = s.stderr();
    assert!(
        err.contains(&format!(
            "[watch] listening repo={} poll_ms=20 debounce_ms=20 max_latency_ms=2000 persist_every=0\n",
            s.repo_str()
        )),
        "{err}"
    );
    assert!(err.contains("[watch] emitted seq=1 nodes="), "{err}");
    assert!(err.contains("[watch] exit reason=stdin_eof"), "{err}");
    assert!(err.contains("[watch] persisted seq=1 reason=exit"), "{err}");
    assert!(!err.contains("reason=interval"), "{err}");

    // The exit persist is the layout a clean build of the edited tree writes
    // (`graph2` sits at the same depth, so the manifest's relative root
    // matches).
    let watched = layout_bytes(&layout);
    assert!(
        watched.contains_key("manifest.json"),
        "{:?}",
        watched.keys()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["build", &s.repo_str(), "--out"])
        .arg(s.repo().join(".glia/graph2"))
        .output()
        .expect("run glia build");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let built = layout_bytes(&s.repo().join(".glia/graph2"));
    assert_eq!(
        watched.keys().collect::<Vec<_>>(),
        built.keys().collect::<Vec<_>>(),
        "same files"
    );
    for (name, bytes) in &watched {
        assert!(bytes == &built[name], "{name} differs from a clean build");
    }
}

#[cfg(unix)]
#[test]
fn sigint_persists_and_exits_0() {
    let s = Scratch::new("sigint");
    let mut w = Watch::spawn(&s, &["--poll-ms", "20", "--persist-every", "0"]);
    ready(&mut w, &s);
    let pid = w.child.id().to_string();
    let kill = Command::new("kill")
        .args(["-INT", &pid])
        .status()
        .expect("run kill");
    assert!(kill.success());
    assert_eq!(w.wait(&s), 0, "stderr:\n{}", s.stderr());
    assert!(s.repo().join(".glia/graph/manifest.json").is_file());
    assert!(s.repo().join(".glia/graph/parse_cache.bin").is_file());
    let err = s.stderr();
    assert!(err.contains("[watch] exit reason=sigint"), "{err}");
    assert!(err.contains("[watch] persisted seq=0 reason=exit"), "{err}");
}

#[test]
fn persists_on_the_interval() {
    let s = Scratch::new("interval");
    let mut w = Watch::spawn(&s, &["--poll-ms", "20", "--persist-every", "1"]);
    ready(&mut w, &s);
    let manifest = s.repo().join(".glia/graph/manifest.json");
    let started = Instant::now();
    while !manifest.is_file() {
        assert!(
            w.child.try_wait().expect("try_wait").is_none(),
            "watch exited before the interval persist; stderr:\n{}",
            s.stderr()
        );
        assert!(
            started.elapsed() < TIMEOUT,
            "no interval persist; stderr:\n{}",
            s.stderr()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        w.child.try_wait().expect("try_wait").is_none(),
        "still running"
    );
    drop(w.stdin.take());
    assert_eq!(w.wait(&s), 0, "stderr:\n{}", s.stderr());
    let err = s.stderr();
    assert!(
        err.contains("[watch] persisted seq=0 reason=interval"),
        "{err}"
    );
    assert!(err.contains("[watch] persisted seq=0 reason=exit"), "{err}");
}

#[test]
fn refuses_no_overlay() {
    let s = Scratch::new("no-overlay");
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["--no-overlay", "watch", &s.repo_str()])
        .stdin(Stdio::null())
        .output()
        .expect("run glia watch");
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "error: --no-overlay: watch persists into the default gmap dir, which holds the overlay-applied graph"
        ),
        "{err}"
    );
    assert!(out.stdout.is_empty());
    assert!(!s.repo().join(".glia").exists(), "nothing written");
}

#[test]
fn missing_dir_exits_2() {
    let s = Scratch::new("missing");
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .arg("watch")
        .arg(s.0.join("nope"))
        .stdin(Stdio::null())
        .output()
        .expect("run glia watch");
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("error: not a directory:"), "{err}");
    assert!(out.stdout.is_empty());
    assert!(!s.0.join("nope").exists());
}

/// GA.11: `--with` watches both repos as one joint build: the ready line
/// lists them, an edit in the second repo is one update naming its file
/// `<label>:<rel>`, and the exit persist writes the joint layout to `--out`,
/// never to either repo's own `.glia/graph` (each repo's parse cache is saved
/// beside its own root, as a single-repo watch saves it).
#[test]
fn with_repos_persist_to_out() {
    let s = Scratch::new("with");
    let second = s.second();
    let out = s.0.join("joint");
    let out_str = out.to_str().expect("utf-8 scratch path").to_string();
    let mut w = Watch::spawn(
        &s,
        &[
            "--poll-ms",
            "20",
            "--debounce-ms",
            "20",
            "--persist-every",
            "0",
            "--with",
            &second,
            "--out",
            &out_str,
        ],
    );
    let r = ready(&mut w, &s);
    assert_eq!(
        r["repos"],
        serde_json::json!([s.repo_str(), second.clone()]),
        "{r}"
    );
    let staged = s.0.join("b.py.new");
    std::fs::write(&staged, B_PY_EDITED).expect("stage the edit");
    std::fs::rename(&staged, s.0.join("repo2/b.py")).expect("edit b.py");
    let u = w.next_json(&s, "the update");
    assert_eq!(u["seq"], 1, "{u}");
    assert_eq!(
        u["files"]["modified"],
        serde_json::json!([format!("{second}:b.py")]),
        "{u}"
    );
    assert!(u["total_nodes"].as_u64().is_some_and(|n| n > 0), "{u}");
    drop(w.stdin.take());
    assert_eq!(w.wait(&s), 0, "stderr:\n{}", s.stderr());
    let err = s.stderr();
    assert!(
        err.contains(&format!(
            "[watch] listening repo={} poll_ms=20 debounce_ms=20 max_latency_ms=2000 persist_every=0 repos=2\n",
            s.repo_str()
        )),
        "{err}"
    );
    assert!(
        err.contains(&format!(
            "[watch] persisted seq=1 reason=exit out={out_str}\n"
        )),
        "{err}"
    );
    assert!(out.join("manifest.json").is_file(), "the joint layout");
    for root in [s.repo(), s.0.join("repo2")] {
        let own = root.join(".glia/graph");
        assert!(
            !own.join("manifest.json").exists(),
            "no layout in {}",
            own.display()
        );
        assert!(
            own.join("parse_cache.bin").is_file(),
            "the parse cache beside {}",
            root.display()
        );
    }
}

/// GA.11: `--with` without `--out` exits 2 before anything is read or
/// written.
#[test]
fn with_needs_out() {
    let s = Scratch::new("with-no-out");
    let second = s.second();
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["watch", &s.repo_str(), "--with", &second])
        .stdin(Stdio::null())
        .output()
        .expect("run glia watch");
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "error: --with needs --out; a joint layout never overwrites a repo's own .glia/graph"
        ),
        "{err}"
    );
    assert!(!err.contains("[watch] open"), "nothing opened: {err}");
    assert!(out.stdout.is_empty());
    assert!(!s.repo().join(".glia").exists(), "nothing written");
    assert!(!s.0.join("repo2/.glia").exists(), "nothing written");
}

/// GA.11, the debounce cap (0.5.2 W9's DC.8 follow-up): a file rewritten
/// every 50 ms never stays still for `--debounce-ms`, so before the cap it was
/// never applied while it kept moving; with `--max-latency-ms 500` the pending
/// set is applied once it is that old, while the writer still runs.
#[test]
fn max_latency_applies_a_moving_path() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let s = Scratch::new("latency");
    let mut w = Watch::spawn(
        &s,
        &[
            "--poll-ms",
            "20",
            "--debounce-ms",
            "200",
            "--max-latency-ms",
            "500",
            "--persist-every",
            "0",
        ],
    );
    ready(&mut w, &s);
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = Arc::clone(&stop);
        let dir = s.0.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut i = 0usize;
            // Bounded: a hung watch never leaves the writer running.
            while !stop.load(Ordering::SeqCst) && started.elapsed() < TIMEOUT {
                // Two edits of different lengths, neither the opened text,
                // renamed in so no scan reads a half-written file.
                let text = if i.is_multiple_of(2) {
                    A_PY_EDITED.to_string()
                } else {
                    format!("{A_PY_EDITED}\n# {i}\n")
                };
                let staged = dir.join("a.py.new");
                let _ = std::fs::write(&staged, text);
                let _ = std::fs::rename(&staged, dir.join("repo/a.py"));
                i += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    };
    let started = Instant::now();
    let line = w.lines.recv_timeout(Duration::from_secs(8));
    let waited = started.elapsed();
    let still_writing = !writer.is_finished();
    stop.store(true, Ordering::SeqCst);
    let _ = writer.join();
    let line = line.unwrap_or_else(|e| {
        let _ = w.child.kill();
        panic!(
            "no update within 8 s of a path moving every 50 ms ({e}); stderr:\n{}",
            s.stderr()
        )
    });
    assert!(still_writing, "applied while the path was still moving");
    let u: Value = serde_json::from_str(&line).expect("an update line");
    assert_eq!(u["seq"], 1, "{u}");
    assert_eq!(u["files"]["modified"], serde_json::json!(["a.py"]), "{u}");
    drop(w.stdin.take());
    assert_eq!(w.wait(&s), 0, "stderr:\n{}", s.stderr());
    let err = s.stderr();
    assert!(
        err.contains(" max_latency_ms=500 persist_every=0\n"),
        "{err}"
    );
    eprintln!("[ga11-test] first update after {} ms", waited.as_millis());
}
