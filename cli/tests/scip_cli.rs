//! CE.1c — `glia scip import`, the CLI surface of the SCIP snapshot step,
//! driving the real binary over temporary repos with the committed indexes in
//! `cli/tests/data/scip/` (protoc-encoded from the `.textproto` beside each;
//! the decoder's own tests re-encode them).
//!
//! The `[scip] decoded index=... ` stderr line is the fired_on marker;
//! asserting it here makes it a tested contract. Grep it with
//! `cargo test -p glia-cli --test scip_cli -- --nocapture 2>&1 | grep -o '\[scip\] decoded .*'`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use glia_code_domain::snapshots::{ScipSnapshot, read_scip, scip_dir};

/// CE.1b's probe sources (snapshots/tests/scip_import.rs REPOS_PY /
/// HANDLERS_PY), which probe.scip indexes.
const REPOS_PY: &str = "class UserRepo:\n    def save(self, row):\n        return row\n\n\nclass OrderRepo:\n    def save(self, row):\n        return row\n";
const HANDLERS_PY: &str = "from svc.repos import UserRepo\n\n\ndef handle(row):\n    repo = UserRepo()\n    return repo.save(row)\n";
/// The source utf16.scip indexes.
const UTF16_APP_TS: &str = "function f🚀(x: number) { return x; }\nexport const é = f🚀(1);\n";

const USER_SAVE: &str = "scip-python python svc 0.1 `svc.repos`/UserRepo#save().";
const REPOSITORY_SAVE: &str = "scip-python python repokit 1.2 `repokit.base`/Repository#save().";

/// A scratch repo under the temp dir, removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`).
struct Scratch {
    root: PathBuf,
    top: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-ce1c-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        std::fs::create_dir_all(&top).expect("scratch dir");
        Scratch { root, top }
    }

    fn probe(name: &str) -> Self {
        let s = Scratch::new(name);
        s.write("svc/repos.py", REPOS_PY);
        s.write("svc/handlers.py", HANDLERS_PY);
        s
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.top.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write source");
    }

    /// Write `bytes` beside the repo (never inside it) and return the path.
    fn index(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, bytes).expect("write index");
        path
    }

    fn snapshot(&self) -> ScipSnapshot {
        read_scip(&self.top).expect("a complete scip snapshot")
    }
}

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/scip")
        .join(name)
}

fn glia(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .output()
        .expect("run glia")
}

fn import(repo: &Path, index: &Path, prefix: Option<&str>) -> Output {
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "scip".as_ref(),
        "import".as_ref(),
        repo.as_os_str(),
        index.as_os_str(),
    ];
    if let Some(p) = prefix {
        args.push("--prefix".as_ref());
        args.push(p.as_ref());
    }
    glia(&args)
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The one stderr line starting with `prefix`.
fn line<'a>(text: &'a str, prefix: &str) -> &'a str {
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with(prefix)).collect();
    assert_eq!(lines.len(), 1, "one `{prefix}` line expected in:\n{text}");
    lines[0]
}

#[test]
fn import_writes_the_snapshot() {
    let s = Scratch::probe("probe");
    let out = import(&s.top, &data("probe.scip"), Some("."));
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{err}");

    assert_eq!(
        line(&err, "[scip] decoded "),
        "[scip] decoded index=probe.scip documents=2 occurrences=3 symbols=1 external_symbols=1 \
         unknown_fields=0 malformed_ranges=0"
    );
    let imported = line(&err, "[scip] import repo=");
    for want in [
        "tool=scip-python@0.6.0",
        "documents=2",
        "defs=2 refs=1 calls=1",
        "bad_ranges=0",
        "surface=cli",
    ] {
        assert!(imported.contains(want), "`{want}` missing from: {imported}");
    }

    let snap = s.snapshot();
    let paths: Vec<&str> = snap.documents.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, ["svc/handlers.py", "svc/repos.py"]);
    let handlers = &snap.documents[0];
    assert_eq!(handlers.refs.len(), 1);
    assert_eq!((handlers.refs[0].line, handlers.refs[0].call), (5, true));
    let repos = &snap.documents[1];
    let defs: Vec<(u32, &str)> = repos
        .defs
        .iter()
        .map(|d| (d.line, d.name.as_str()))
        .collect();
    assert_eq!(defs, [(1, "save"), (6, "save")]);

    // The is_implementation relationship reached the snapshot; the
    // external symbol's own SymbolInformation was skipped, its id kept only
    // as the target.
    let id = |symbol: &str| {
        snap.symbols
            .iter()
            .find(|r| r.symbol == symbol)
            .map(|r| r.id)
    };
    let user_save = snap
        .symbols
        .iter()
        .find(|r| r.symbol == USER_SAVE)
        .expect("UserRepo#save kept");
    assert_eq!(
        user_save.implements,
        [id(REPOSITORY_SAVE).expect("the implements target is kept")]
    );
    assert_eq!(snap.symbols.len(), 3);

    let text = stdout(&out);
    assert_eq!(
        text,
        format!(
            "wrote 2 documents, 3 symbols -> {}\nrun `glia build {}` to ingest.\n",
            scip_dir(&s.top).display(),
            s.top.display()
        )
    );
}

#[test]
fn utf16_index_imports() {
    let s = Scratch::new("utf16");
    s.write("web/app.ts", UTF16_APP_TS);
    let out = import(&s.top, &data("utf16.scip"), Some("."));
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "stderr:\n{err}");
    assert_eq!(
        line(&err, "[scip] decoded "),
        "[scip] decoded index=utf16.scip documents=1 occurrences=1 symbols=0 external_symbols=0 \
         unknown_fields=0 malformed_ranges=0"
    );
    let imported = line(&err, "[scip] import repo=");
    for want in [
        "refs=1 calls=1",
        "bad_ranges=0",
        "encoding_unspecified=0",
        "surface=cli",
    ] {
        assert!(imported.contains(want), "`{want}` missing from: {imported}");
    }
    let snap = s.snapshot();
    assert_eq!(snap.documents.len(), 1);
    let refs: Vec<(u32, bool)> = snap.documents[0]
        .refs
        .iter()
        .map(|r| (r.line, r.call))
        .collect();
    assert_eq!(
        refs,
        [(1, true)],
        "`f🚀(1)` after `é` is a call, read in UTF-16 code units"
    );
}

#[test]
fn undecodable_index_writes_nothing() {
    let probe = std::fs::read(data("probe.scip")).expect("probe.scip");
    // The metadata field is bytes 0..50 and the first document 50..346; cut inside it.
    let s = Scratch::probe("truncated");
    let cut = s.index("cut.scip", &probe[..200]);
    let out = import(&s.top, &cut, Some("."));
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "stderr:\n{err}");
    assert!(
        err.contains("error: ") && err.contains("truncated Index.documents"),
        "{err}"
    );
    assert!(err.contains("nothing written"), "{err}");
    assert!(
        !err.contains("[scip] decoded"),
        "no decoded marker for a failed decode:\n{err}"
    );
    assert!(
        !scip_dir(&s.top).exists(),
        "no snapshot dir after a failed decode"
    );
    assert!(!s.top.join(".glia").exists(), "nothing under .glia either");

    // Documents before the metadata (the metadata field moved to the end).
    let s = Scratch::probe("reordered");
    let reordered = s.index("reordered.scip", &[&probe[50..], &probe[..50]].concat());
    let out = import(&s.top, &reordered, Some("."));
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "stderr:\n{err}");
    assert!(err.contains("metadata must come first"), "{err}");
    assert!(!scip_dir(&s.top).exists());

    // A clean decode that keeps no document: the sources are absent.
    let s = Scratch::new("empty-repo");
    let out = import(&s.top, &data("probe.scip"), Some("."));
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "stderr:\n{err}");
    assert!(
        err.contains("[scip] decoded index=probe.scip documents=2"),
        "{err}"
    );
    assert!(err.contains("no document of the index was kept"), "{err}");
    assert!(!scip_dir(&s.top).exists());
}

#[test]
fn bad_inputs_exit_one_and_usage_exits_two() {
    let s = Scratch::probe("bad-inputs");
    let out = import(&s.top, &s.root.join("missing.scip"), Some("."));
    assert_eq!(
        out.status.code(),
        Some(1),
        "missing index: {}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("missing.scip"), "{}", stderr(&out));

    let out = import(&s.top, &data("probe.scip"), Some("../elsewhere"));
    assert_eq!(
        out.status.code(),
        Some(1),
        "a `..` prefix: {}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("--prefix"), "{}", stderr(&out));

    let out = import(&s.root.join("no-such-repo"), &data("probe.scip"), Some("."));
    assert_eq!(out.status.code(), Some(1), "missing repo: {}", stderr(&out));
    assert!(!scip_dir(&s.top).exists());

    let out = glia(&["scip".as_ref(), "import".as_ref(), s.top.as_os_str()]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "no index argument is a usage error"
    );
}
