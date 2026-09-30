//! CE.2e — `glia cache push|pull` over the HTTP object store, driven through
//! the real binary against an in-test HTTP/1.1 server on 127.0.0.1 (std
//! only: GET / HEAD / PUT into a map, 404 for an unknown key, 401 without the
//! expected bearer token, one request per connection).
//!
//! The fixture repo is cache_cli.rs's: four files a build hands a language
//! parser, each side a copy at `<tmp>/<side>/repo` (so the sides share keys).
//! The fired_on marker is grep-able:
//! `cargo test -p glia-cli --test cache_http -- --nocapture 2>&1 | grep -o '\[cache\] store=.*'`

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glia_engine::BUILD_STAMP;

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
const TOKEN: &str = "t0k";

/// A scratch dir, created fresh and removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-ce2e-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    fn repo(&self, side: &str) -> PathBuf {
        let repo = self.0.join(side).join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir");
        for (name, text) in FILES {
            std::fs::write(repo.join(name), text).expect("write fixture file");
        }
        repo
    }

    fn key_file(&self) -> PathBuf {
        let path = self.0.join("k.hex");
        std::fs::write(&path, format!("{KEY}\n")).expect("write key file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("chmod key file");
        }
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// The in-test object server.
// ---------------------------------------------------------------------------

/// One request the server saw: method, target and the two headers checked.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    authorization: Option<String>,
    user_agent: Option<String>,
}

#[derive(Default)]
struct State {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    seen: Mutex<Vec<Seen>>,
    /// Answer every GET with 500.
    fail_get: AtomicBool,
}

struct Server {
    port: u16,
    state: Arc<State>,
}

impl Server {
    /// Listen on 127.0.0.1:0; objects live under `/glia/`.
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let port = listener.local_addr().expect("local addr").port();
        let state = Arc::new(State::default());
        let st = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let st = Arc::clone(&st);
                std::thread::spawn(move || {
                    let _ = handle(stream, &st);
                });
            }
        });
        Server { port, state }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/glia", self.port)
    }

    fn seen(&self) -> Vec<Seen> {
        self.state.seen.lock().expect("seen").clone()
    }

    fn keys(&self) -> Vec<String> {
        self.state
            .objects
            .lock()
            .expect("objects")
            .keys()
            .cloned()
            .collect()
    }
}

fn handle(stream: TcpStream, st: &State) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let mut headers = BTreeMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim_end().is_empty() {
            break;
        }
        if let Some((k, v)) = h.trim_end().split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    let authorization = headers.get("authorization").cloned();
    st.seen.lock().expect("seen").push(Seen {
        method: method.clone(),
        target: target.clone(),
        authorization: authorization.clone(),
        user_agent: headers.get("user-agent").cloned(),
    });

    let key = target.strip_prefix("/glia/").map(str::to_string);
    let (code, reason, answer): (u16, &str, Vec<u8>) =
        if authorization.as_deref() != Some(&format!("Bearer {TOKEN}")) {
            (401, "Unauthorized", b"no token".to_vec())
        } else if let Some(key) = key {
            let mut objects = st.objects.lock().expect("objects");
            match method.as_str() {
                "GET" if st.fail_get.load(Ordering::Relaxed) => {
                    (500, "Internal Server Error", b"boom".to_vec())
                }
                "GET" | "HEAD" => match objects.get(&key) {
                    Some(o) => (200, "OK", o.clone()),
                    None => (404, "Not Found", b"not found".to_vec()),
                },
                "PUT" => {
                    objects.insert(key, body);
                    (201, "Created", Vec::new())
                }
                _ => (405, "Method Not Allowed", Vec::new()),
            }
        } else {
            (404, "Not Found", b"not found".to_vec())
        };
    let mut out = stream;
    write!(
        out,
        "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        answer.len()
    )?;
    if method != "HEAD" {
        out.write_all(&answer)?;
    }
    out.flush()
}

// ---------------------------------------------------------------------------
// Running the binary.
// ---------------------------------------------------------------------------

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 temp path")
}

/// Run the real binary with no cache key or token in its environment,
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

/// `glia build <repo>`, persisting: writes the layout and the parse cache.
fn build(repo: &Path) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(["build", s(repo)])
        .env_remove("GLIA_NO_PERSIST")
        .env_remove("GLIA_CACHE_KEY")
        .env_remove("GLIA_CACHE_TOKEN")
        .output()
        .expect("run glia build");
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
        .filter_map(|kv| kv.trim_matches(['(', ')']).split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

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

/// The one `[cache] store=` line of a run.
fn store_line(run: &Run) -> String {
    let lines: Vec<&str> = run
        .stderr
        .lines()
        .filter(|l| l.starts_with("[cache] store="))
        .collect();
    assert_eq!(lines.len(), 1, "{}", run.stderr);
    lines[0].to_string()
}

fn sidecar(repo: &Path) -> PathBuf {
    repo.join(".glia/graph/parse_cache.bin")
}

#[test]
fn https_store_over_loopback() {
    let t = Scratch::new("round");
    let (a, b) = (t.repo("a"), t.repo("b"));
    let key = t.key_file();
    let server = Server::start();
    let url = server.url();
    let origin = format!("http://127.0.0.1:{}", server.port);
    build(&a);

    let push = glia(
        &["cache", "push", s(&a), &url, "--key-file", s(&key)],
        &[("GLIA_CACHE_TOKEN", TOKEN)],
    );
    assert_eq!(push.code, 0, "{}", push.stderr);
    assert_fields(
        &marker(&push, "[cache] push "),
        &[
            ("store", url.as_str()),
            ("entries", "4"),
            ("uploaded", "4"),
            ("present", "0"),
            ("signed", "yes"),
        ],
        &push,
    );
    // Four HEADs answered 404, then four PUTs.
    assert_eq!(
        store_line(&push),
        format!(
            "[cache] store={origin} transport=http-loopback requests=8 (get=0 head=4 put=4) status_4xx=4 status_5xx=0"
        )
    );
    let keys = server.keys();
    assert_eq!(keys.len(), 4, "{keys:?}");
    let prefix = format!("v1/{BUILD_STAMP}/");
    assert!(
        keys.iter()
            .all(|k| k.starts_with(&prefix) && k.ends_with(".gpc")),
        "{keys:?}"
    );

    let pull = glia(
        &["cache", "pull", s(&b), &url, "--key-file", s(&key)],
        &[("GLIA_CACHE_TOKEN", TOKEN)],
    );
    assert_eq!(pull.code, 0, "{}", pull.stderr);
    assert_fields(
        &marker(&pull, "[cache] pull "),
        &[
            ("store", url.as_str()),
            ("files", "4"),
            ("fetched", "4"),
            ("missing", "0"),
            ("rejected", "0"),
        ],
        &pull,
    );
    let m = marker(&pull, "[cache] store=");
    assert_fields(
        &m,
        &[
            ("transport", "http-loopback"),
            ("requests", "4"),
            ("get", "4"),
            ("head", "0"),
            ("put", "0"),
            ("status_4xx", "0"),
            ("status_5xx", "0"),
        ],
        &pull,
    );
    assert!(sidecar(&b).is_file(), "the pull wrote no sidecar");
    let warm = build(&b);
    assert!(
        warm.stderr.contains("reused 4, reparsed 0, evicted 0"),
        "{}",
        warm.stderr
    );

    // Every request carried the token and glia's user agent; no output did.
    let seen = server.seen();
    assert_eq!(seen.len(), 12, "{seen:?}");
    for r in &seen {
        assert_eq!(r.authorization.as_deref(), Some("Bearer t0k"), "{r:?}");
        assert!(
            r.user_agent
                .as_deref()
                .is_some_and(|u| u.starts_with("glia/")),
            "{r:?}"
        );
        assert!(r.target.starts_with("/glia/v1/"), "{r:?}");
    }
    for run in [&push, &pull] {
        assert!(
            !run.stderr.contains(TOKEN) && !run.stdout.contains(TOKEN),
            "the token was echoed:\n{}",
            run.stderr
        );
    }

    // Without the token the server answers 401: a store failure, exit 1.
    let denied = glia(&["cache", "push", s(&a), &url, "--key-file", s(&key)], &[]);
    assert_eq!(denied.code, 1, "{}", denied.stderr);
    assert!(denied.stderr.contains("HTTP 401"), "{}", denied.stderr);
    assert_fields(
        &marker(&denied, "[cache] store="),
        &[("requests", "1"), ("head", "1"), ("status_4xx", "1")],
        &denied,
    );
}

#[test]
fn a_5xx_is_a_store_failure() {
    let t = Scratch::new("5xx");
    let b = t.repo("b");
    let server = Server::start();
    server.state.fail_get.store(true, Ordering::Relaxed);
    let url = server.url();

    let pull = glia(
        &["cache", "pull", s(&b), &url, "--unsigned"],
        &[("GLIA_CACHE_TOKEN", TOKEN)],
    );
    assert_eq!(pull.code, 1, "{}", pull.stderr);
    assert!(
        pull.stderr.contains("HTTP 500") && pull.stderr.contains("nothing written"),
        "{}",
        pull.stderr
    );
    assert!(!pull.stderr.contains("[cache] pull "), "{}", pull.stderr);
    assert!(!sidecar(&b).exists(), "a failed pull wrote the sidecar");
    assert!(!b.join(".glia").exists(), "a failed pull wrote under .glia");
    assert_fields(
        &marker(&pull, "[cache] store="),
        &[
            ("requests", "4"),
            ("get", "4"),
            ("status_4xx", "0"),
            ("status_5xx", "4"),
        ],
        &pull,
    );
    assert!(
        server.seen().iter().all(|r| r.method == "GET"),
        "{:?}",
        server.seen()
    );
}

// ---------------------------------------------------------------------------
// `--layout` travels through the HTTP store unchanged (parts are ordinary
// objects). Needs git; without it the test prints a note and passes.
// ---------------------------------------------------------------------------

/// One hermetic git command in `dir`; panics on failure.
fn git(t: &Scratch, dir: &Path, args: &[&str]) {
    let home = t.0.join("home");
    std::fs::create_dir_all(&home).expect("git home");
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=glia",
            "-c",
            "user.email=glia@example.invalid",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", &home)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn layout_travels_over_http() {
    if !Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("note: no git binary on PATH; skipping the --layout test");
        return;
    }
    let t = Scratch::new("layout");
    let origin = t.0.join("origin");
    std::fs::create_dir_all(&origin).expect("origin dir");
    git(&t, &origin, &["init", "-q"]);
    for (name, text) in FILES {
        std::fs::write(origin.join(name), text).expect("write fixture file");
    }
    git(&t, &origin, &["add", "-A"]);
    git(&t, &origin, &["commit", "-q", "-m", "fixture"]);
    let sides: Vec<PathBuf> = ["a", "b"]
        .iter()
        .map(|side| {
            let dest = t.0.join(side).join("repo");
            std::fs::create_dir_all(dest.parent().expect("side dir")).expect("side dir");
            git(&t, &t.0, &["clone", "-q", s(&origin), s(&dest)]);
            dest
        })
        .collect();
    let (a, b) = (&sides[0], &sides[1]);
    let key = t.key_file();
    let server = Server::start();
    let url = server.url();
    build(a);

    let token = [("GLIA_CACHE_TOKEN", TOKEN)];
    let push = glia(
        &[
            "cache",
            "push",
            s(a),
            &url,
            "--key-file",
            s(&key),
            "--layout",
        ],
        &token,
    );
    assert_eq!(push.code, 0, "{}", push.stderr);
    assert_fields(
        &marker(&push, "[cache] layout push "),
        &[("result", "pushed")],
        &push,
    );
    let layout_prefix = format!("v1/{BUILD_STAMP}/layout/");
    assert_eq!(
        server
            .keys()
            .iter()
            .filter(|k| k.starts_with(&layout_prefix) && k.ends_with(".gla"))
            .count(),
        1,
        "{:?}",
        server.keys()
    );

    let pull = glia(
        &[
            "cache",
            "pull",
            s(b),
            &url,
            "--key-file",
            s(&key),
            "--layout",
        ],
        &token,
    );
    assert_eq!(pull.code, 0, "{}", pull.stderr);
    assert_fields(
        &marker(&pull, "[cache] layout pull "),
        &[("result", "hit")],
        &pull,
    );
    assert!(b.join(".glia/graph/manifest.json").is_file());
    assert_fields(
        &marker(&pull, "[cache] store="),
        &[("status_5xx", "0")],
        &pull,
    );
}
