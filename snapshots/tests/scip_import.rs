//! CE.1b: the SCIP importer against temporary repos, fed hand-built
//! documents (no protobuf; the CLI's decoder, CE.1c, fills the same types).

use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::snapshots::{
    META_FILE, SCIP_DOCUMENTS_FILE, SCIP_SYMBOLS_FILE, ScipRefRow, ScipSnapshot, read_scip, scip_dir, source_hash,
};
use glia_snapshots::{
    PositionEncoding, ScipDocumentIn, ScipImportOptions, ScipImportSummary, ScipImporter, ScipIndexInfo,
    ScipOccurrenceIn, ScipSymbolIn, next_is_call, unit_offset_to_byte,
};

const USER_SAVE: &str = "scip-python python svc 0.1 `svc.repos`/UserRepo#save().";
const ORDER_SAVE: &str = "scip-python python svc 0.1 `svc.repos`/OrderRepo#save().";
const USER_REPO: &str = "scip-python python svc 0.1 `svc.repos`/UserRepo#";
const BASE_REPO: &str = "scip-python python svc 0.1 `svc.base`/Repo#";
const DEFINITION: i32 = 0x1;
const IMPORT: i32 = 0x2;
const WRITE: i32 = 0x4;
const READ: i32 = 0x8;
const FORWARD: i32 = 0x40;

const REPOS_PY: &str = "class UserRepo:\n    def save(self, row):\n        return row\n\n\nclass OrderRepo:\n    def save(self, row):\n        return row\n";
const HANDLERS_PY: &str =
    "from svc.repos import UserRepo\n\n\ndef handle(row):\n    repo = UserRepo()\n    return repo.save(row)\n";

/// A temporary repo at `<tmp>/repo`, with room beside it for "outside" files.
struct Repo {
    tmp: tempfile::TempDir,
    top: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let top = tmp.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        Repo { tmp, top }
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.top.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn uri(&self) -> String {
        format!("file://{}", std::fs::canonicalize(&self.top).unwrap().display())
    }

    fn info(&self) -> ScipIndexInfo {
        ScipIndexInfo { tool: "scip-python".into(), tool_version: "0.6.0".into(), project_root: self.uri() }
    }

    fn snapshot_bytes(&self) -> [Vec<u8>; 3] {
        [SCIP_DOCUMENTS_FILE, SCIP_SYMBOLS_FILE, META_FILE].map(|f| std::fs::read(scip_dir(&self.top).join(f)).unwrap())
    }
}

fn probe_repo() -> Repo {
    let repo = Repo::new();
    repo.write("svc/repos.py", REPOS_PY);
    repo.write("svc/handlers.py", HANDLERS_PY);
    repo
}

fn occ(line: i32, start: i32, end: i32, symbol: &str, roles: i32) -> ScipOccurrenceIn {
    ScipOccurrenceIn {
        start_line: line,
        start_char: start,
        end_line: line,
        end_char: end,
        symbol: symbol.into(),
        roles,
    }
}

fn doc(path: &str, encoding: PositionEncoding, occurrences: Vec<ScipOccurrenceIn>) -> ScipDocumentIn {
    ScipDocumentIn {
        relative_path: path.into(),
        language: "python".into(),
        encoding,
        occurrences,
        symbols: Vec::new(),
    }
}

/// The CE.1b probe: two `save` definitions in repos.py, one call of
/// `UserRepo#save` in handlers.py.
fn probe_documents() -> Vec<ScipDocumentIn> {
    vec![
        doc(
            "svc/repos.py",
            PositionEncoding::Utf8,
            vec![occ(1, 8, 12, USER_SAVE, DEFINITION), occ(6, 8, 12, ORDER_SAVE, DEFINITION)],
        ),
        doc("svc/handlers.py", PositionEncoding::Utf8, vec![occ(5, 16, 20, USER_SAVE, READ)]),
    ]
}

fn opts() -> ScipImportOptions {
    ScipImportOptions::default()
}

fn import(
    root: &Path,
    info: ScipIndexInfo,
    documents: Vec<ScipDocumentIn>,
    opts: ScipImportOptions,
) -> Result<ScipImportSummary, String> {
    let mut importer = ScipImporter::new(root, info, opts)?;
    for d in documents {
        importer.document(d);
    }
    importer.finish()
}

fn snapshot(root: &Path) -> ScipSnapshot {
    read_scip(root).expect("a complete scip snapshot")
}

fn symbol_id(snap: &ScipSnapshot, symbol: &str) -> u32 {
    snap.symbols.iter().find(|s| s.symbol == symbol).unwrap_or_else(|| panic!("symbol {symbol} kept")).id
}

fn has_symbol(snap: &ScipSnapshot, symbol: &str) -> bool {
    snap.symbols.iter().any(|s| s.symbol == symbol)
}

/// Set when the test re-executes itself to read the importer's stderr marker.
const CHILD_ENV: &str = "GLIA_SCIP_IMPORT_TEST_CHILD_REPO";

#[test]
fn import_writes_rows_and_symbols() {
    if let Some(dir) = std::env::var_os(CHILD_ENV) {
        // The child run below: import into the parent's repo with stderr uncaptured.
        let root = PathBuf::from(dir);
        let info = ScipIndexInfo {
            tool: "scip-python".into(),
            tool_version: "0.6.0".into(),
            project_root: format!("file://{}", root.display()),
        };
        import(&root, info, probe_documents(), opts()).expect("child import");
        return;
    }

    let repo = probe_repo();
    let summary = import(&repo.top, repo.info(), probe_documents(), opts()).expect("import");
    let snap = snapshot(&repo.top);

    let paths: Vec<&str> = snap.documents.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, ["svc/handlers.py", "svc/repos.py"], "2 documents in path order");
    let symbols: Vec<&str> = snap.symbols.iter().map(|s| s.symbol.as_str()).collect();
    assert_eq!(symbols, [ORDER_SAVE, USER_SAVE], "symbols sorted by string");
    assert_eq!(snap.symbols.iter().map(|s| s.id).collect::<Vec<_>>(), [0, 1]);

    let handlers = &snap.documents[0];
    assert_eq!(
        handlers.refs,
        [ScipRefRow { s: symbol_id(&snap, USER_SAVE), line: 5, call: true, write: false, import: false }]
    );
    assert!(handlers.defs.is_empty());
    assert_eq!(handlers.language, "python");
    assert_eq!(handlers.source_hash, source_hash(HANDLERS_PY.as_bytes()));

    let repos = &snap.documents[1];
    let names: Vec<&str> = repos.defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["save", "save"]);
    let def_rows: Vec<(u32, u32)> = repos.defs.iter().map(|d| (d.line, d.s)).collect();
    assert_eq!(def_rows, [(1, symbol_id(&snap, USER_SAVE)), (6, symbol_id(&snap, ORDER_SAVE))]);
    assert!(repos.refs.is_empty());

    assert_eq!(snap.meta.tool, "scip-python");
    assert_eq!(snap.meta.tool_version, "0.6.0");
    assert_eq!(snap.meta.project_root, "");
    assert_eq!(snap.meta.skipped_documents, 0);

    assert_eq!(
        (summary.documents, summary.skipped, summary.defs, summary.refs, summary.calls, summary.symbols),
        (2, 0, 2, 1, 1, 2)
    );
    assert_eq!(
        (summary.locals, summary.forward, summary.bad_ranges, summary.encoding_unspecified),
        (0, 0, 0, 0)
    );

    // The marker: re-run this test in a child process (libtest output
    // uncaptured) against a fresh copy of the probe, and read its stderr.
    let child = probe_repo();
    let out = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "import_writes_rows_and_symbols", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, std::fs::canonicalize(&child.top).unwrap())
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child import failed:\n{stderr}");
    let markers: Vec<&str> = stderr.lines().filter(|l| l.starts_with("[scip] import repo=")).collect();
    assert_eq!(
        markers,
        ["[scip] import repo=repo tool=scip-python@0.6.0 documents=2 skipped=0 defs=2 refs=1 calls=1 \
          symbols=2 locals=0 forward=0 bad_ranges=0 encoding_unspecified=0 surface=lib"],
        "one marker line on stderr:\n{stderr}"
    );
    assert_eq!(child.snapshot_bytes(), repo.snapshot_bytes(), "the child wrote the same snapshot");
}

#[test]
fn offsets_convert_by_encoding() {
    let line = "const é = f🚀(x)";
    let f = line.find('f').unwrap();
    let rocket = line.find('🚀').unwrap();
    let paren = line.find('(').unwrap();
    assert_eq!((f, rocket, paren), (11, 12, 16));

    use PositionEncoding::{Unspecified, Utf8, Utf16, Utf32};
    assert_eq!(unit_offset_to_byte(line, 10, Utf16), Some(f));
    assert_eq!(unit_offset_to_byte(line, 11, Utf16), Some(rocket));
    assert_eq!(unit_offset_to_byte(line, 12, Utf16), None, "inside the surrogate pair");
    assert_eq!(unit_offset_to_byte(line, 13, Utf16), Some(paren));

    assert_eq!(unit_offset_to_byte(line, 10, Utf32), Some(f));
    assert_eq!(unit_offset_to_byte(line, 11, Utf32), Some(rocket));
    assert_eq!(unit_offset_to_byte(line, 12, Utf32), Some(paren));

    assert_eq!(unit_offset_to_byte(line, 11, Utf8), Some(f));
    assert_eq!(unit_offset_to_byte(line, 7, Utf8), None, "inside é");
    assert_eq!(unit_offset_to_byte(line, 13, Utf8), None, "inside the rocket");
    assert_eq!(unit_offset_to_byte(line, 16, Utf8), Some(paren));

    // An unspecified encoding reads as UTF-16, the LSP default.
    assert_eq!(unit_offset_to_byte(line, 13, Unspecified), Some(paren));

    // The line end converts; one past it, or a negative offset, does not.
    let utf16_len = line.encode_utf16().count() as i32;
    let utf32_len = line.chars().count() as i32;
    assert_eq!(unit_offset_to_byte(line, utf16_len, Utf16), Some(line.len()));
    assert_eq!(unit_offset_to_byte(line, utf32_len, Utf32), Some(line.len()));
    assert_eq!(unit_offset_to_byte(line, line.len() as i32, Utf8), Some(line.len()));
    assert_eq!(unit_offset_to_byte(line, utf16_len + 1, Utf16), None);
    assert_eq!(unit_offset_to_byte(line, -1, Utf8), None);
    assert_eq!(unit_offset_to_byte("", 0, Utf16), Some(0));
    assert_eq!(unit_offset_to_byte("", 1, Utf16), None);

    // Never a panic, and every offset returned is a char boundary.
    for enc in [Unspecified, Utf8, Utf16, Utf32] {
        for units in -1..40 {
            if let Some(byte) = unit_offset_to_byte(line, units, enc) {
                assert!(line.is_char_boundary(byte), "{enc:?} {units} -> {byte}");
            }
        }
    }
    for units in [i32::MIN, i32::MAX] {
        assert_eq!(unit_offset_to_byte(line, units, Utf16), None);
    }
    assert_eq!(PositionEncoding::from_proto(0), Unspecified);
    assert_eq!(PositionEncoding::from_proto(1), Utf8);
    assert_eq!(PositionEncoding::from_proto(2), Utf16);
    assert_eq!(PositionEncoding::from_proto(3), Utf32);
    assert_eq!(PositionEncoding::from_proto(4), Unspecified);
    assert_eq!(PositionEncoding::from_proto(-1), Unspecified);
}

#[test]
fn call_detection() {
    for yes in ["f(", "f (", "f\t(", "f::<T>(", "f<A<B>>(", "f::<Vec<u8>> (x)", "f <T>(", "f(x)"] {
        assert!(next_is_call(yes, 1), "{yes:?} is a call");
    }
    let unbalanced = format!("f<{}", "A".repeat(298));
    assert_eq!(unbalanced.len(), 300);
    let too_long = format!("f<{}>(", "A".repeat(300));
    for no in ["f.x(", "f)", "f", "f ", "f::new(", "f::(", "f<A>", "f<A>.g(", "f = g(", unbalanced.as_str(), too_long.as_str()] {
        assert!(!next_is_call(no, 1), "{no:?} is not a call");
    }
    // A range that ends mid-line in the middle of a word still reads the next char.
    assert!(next_is_call("x = repo.save(row)", 13));
    assert!(!next_is_call("x = repo.save(row)", 8));
    // An end offset off a char boundary or past the line is never sliced.
    assert!(!next_is_call("é(", 1));
    assert!(!next_is_call("f(", 3));
    assert!(next_is_call("é(", 2));
}

#[test]
fn outside_paths_are_skipped() {
    let repo = probe_repo();
    // An "outside" file beside the repo, and in-repo symlinks to it and to repos.py.
    let outside = repo.tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.py"), "def secret():\n    pass\n").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.py"), repo.top.join("svc/link.py")).unwrap();
    std::os::unix::fs::symlink("repos.py", repo.top.join("svc/alias.py")).unwrap();
    std::os::unix::fs::symlink(&outside, repo.top.join("outdir")).unwrap();

    let secret = |n: u32| format!("scip-python python svc 0.1 `outside`/secret{n}().");
    let def = |n: u32| vec![occ(0, 4, 10, &secret(n), DEFINITION)];
    let mut documents = vec![
        doc("../outside/secret.py", PositionEncoding::Utf8, def(1)),
        doc("/etc/hosts", PositionEncoding::Utf8, def(2)),
        doc("svc/link.py", PositionEncoding::Utf8, def(3)),
        doc("outdir/secret.py", PositionEncoding::Utf8, def(4)),
        doc("svc/../../outside/secret.py", PositionEncoding::Utf8, def(5)),
        doc("../../etc/passwd", PositionEncoding::Utf8, def(6)),
        doc("svc/alias.py", PositionEncoding::Utf8, vec![occ(1, 8, 12, USER_SAVE, DEFINITION)]),
        doc("svc/missing.py", PositionEncoding::Utf8, vec![occ(1, 8, 12, USER_SAVE, DEFINITION)]),
        doc("svc", PositionEncoding::Utf8, vec![occ(1, 8, 12, USER_SAVE, DEFINITION)]),
    ];
    documents.extend(probe_documents());
    let summary = import(&repo.top, repo.info(), documents, opts()).expect("import");
    let snap = snapshot(&repo.top);

    let paths: Vec<&str> = snap.documents.iter().map(|d| d.path.as_str()).collect();
    assert_eq!(paths, ["svc/alias.py", "svc/handlers.py", "svc/repos.py"], "the in-repo symlink is followed");
    for n in 1..=6 {
        assert!(!has_symbol(&snap, &secret(n)), "outside document {n} left no symbol");
    }
    assert_eq!(summary.skipped, 8, "6 outside, 1 missing, 1 directory");
    assert_eq!(snap.meta.skipped_documents, 8);
    assert_eq!(summary.documents, 3);
    assert_eq!(summary.defs, 3);
}

#[test]
fn locals_and_forward_defs_are_dropped() {
    let repo = probe_repo();
    let forward = "scip-clang cxx . . `svc/repos.h`/save().";
    let documents = vec![
        doc(
            "svc/repos.py",
            PositionEncoding::Utf8,
            vec![
                occ(1, 8, 12, USER_SAVE, DEFINITION),
                occ(1, 13, 17, "local 3", DEFINITION),
                occ(2, 15, 18, "local 3", READ),
                occ(2, 8, 14, "", READ),
                occ(6, 8, 12, forward, DEFINITION | FORWARD),
                occ(6, 8, 12, forward, FORWARD),
            ],
        ),
        ScipDocumentIn {
            symbols: vec![ScipSymbolIn { symbol: "local 3".into(), implements: vec![USER_SAVE.into()] }],
            ..doc("svc/handlers.py", PositionEncoding::Utf8, vec![occ(5, 16, 20, "local 9", READ)])
        },
    ];
    let summary = import(&repo.top, repo.info(), documents, opts()).expect("import");
    let snap = snapshot(&repo.top);
    assert_eq!((summary.locals, summary.forward), (4, 2));
    assert_eq!(summary.skipped, 1, "handlers.py kept no row");
    let symbols: Vec<&str> = snap.symbols.iter().map(|s| s.symbol.as_str()).collect();
    assert_eq!(symbols, [USER_SAVE]);
    assert!(snap.symbols[0].implements.is_empty());
    assert_eq!(snap.documents.len(), 1);
    assert_eq!(snap.documents[0].defs.len(), 1);
    assert!(snap.documents[0].refs.is_empty());
    assert_eq!((summary.defs, summary.refs), (1, 0));
}

/// Documents with implements relationships, a write, an import, a duplicate
/// path and a reference to a symbol only an implements names.
fn rich_documents() -> Vec<ScipDocumentIn> {
    let mut documents = probe_documents();
    documents.push(ScipDocumentIn {
        symbols: vec![
            ScipSymbolIn {
                symbol: USER_REPO.into(),
                implements: vec![BASE_REPO.into(), BASE_REPO.into(), "local 1".into(), String::new()],
            },
            ScipSymbolIn { symbol: USER_SAVE.into(), implements: vec![format!("{BASE_REPO}save().")] },
        ],
        ..doc(
            "svc/./repos.py",
            PositionEncoding::Utf16,
            vec![occ(0, 6, 14, USER_REPO, DEFINITION), occ(2, 15, 18, USER_REPO, READ | WRITE)],
        )
    });
    documents.push(doc(
        "svc/handlers.py",
        PositionEncoding::Utf32,
        vec![occ(0, 22, 30, USER_REPO, IMPORT), occ(4, 11, 19, USER_REPO, READ), occ(5, 16, 20, USER_SAVE, READ)],
    ));
    documents
}

#[test]
fn symbol_ids_do_not_depend_on_input_order() {
    let forward = probe_repo();
    let reverse = probe_repo();
    let a = import(&forward.top, forward.info(), rich_documents(), opts()).expect("import");
    let mut reversed = rich_documents();
    reversed.reverse();
    for d in &mut reversed {
        d.occurrences.reverse();
        for s in &mut d.symbols {
            s.implements.reverse();
        }
        d.symbols.reverse();
    }
    let b = import(&reverse.top, reverse.info(), reversed, opts()).expect("import");
    assert_eq!(a, b, "same summary");
    assert_eq!(forward.snapshot_bytes(), reverse.snapshot_bytes(), "byte-identical snapshot files");

    let snap = snapshot(&forward.top);
    let symbols: Vec<&str> = snap.symbols.iter().map(|s| s.symbol.as_str()).collect();
    let mut sorted = symbols.clone();
    sorted.sort();
    assert_eq!(symbols, sorted);
    let base = symbol_id(&snap, BASE_REPO);
    let base_save = symbol_id(&snap, &format!("{BASE_REPO}save()."));
    let user_repo = &snap.symbols[symbol_id(&snap, USER_REPO) as usize];
    assert_eq!(user_repo.implements, [base], "sorted, deduped, locals dropped");
    assert_eq!(snap.symbols[symbol_id(&snap, USER_SAVE) as usize].implements, [base_save]);
    // The duplicate path merged into one document.
    assert_eq!(snap.documents.len(), 2);
    assert_eq!((a.documents, a.skipped, a.defs, a.refs, a.calls), (2, 0, 3, 5, 3));
    let handlers = &snap.documents[0];
    let user_repo_id = symbol_id(&snap, USER_REPO);
    let user_save_id = symbol_id(&snap, USER_SAVE);
    assert_eq!(
        handlers.refs,
        [
            ScipRefRow { s: user_repo_id, line: 0, call: false, write: false, import: true },
            ScipRefRow { s: user_repo_id, line: 4, call: true, write: false, import: false },
            ScipRefRow { s: user_save_id, line: 5, call: true, write: false, import: false },
            ScipRefRow { s: user_save_id, line: 5, call: true, write: false, import: false },
        ]
    );
    let repos = &snap.documents[1];
    assert_eq!(repos.refs, [ScipRefRow { s: user_repo_id, line: 2, call: false, write: true, import: false }]);
    let defs: Vec<(u32, &str)> = repos.defs.iter().map(|d| (d.line, d.name.as_str())).collect();
    assert_eq!(defs, [(0, "UserRepo"), (1, "save"), (6, "save")]);
}

#[test]
fn snapshot_dir_ignores_itself() {
    let fresh = probe_repo();
    import(&fresh.top, fresh.info(), probe_documents(), opts()).expect("import");
    let own = std::fs::read_to_string(scip_dir(&fresh.top).join(".gitignore")).unwrap();
    assert_eq!(own, "# written by glia scip import - regenerable\n*\n");
    assert!(own.lines().any(|l| l == "*"));
    let control = std::fs::read_to_string(fresh.top.join(".glia/.gitignore")).unwrap();
    assert!(control.lines().any(|l| l == "scip-snapshot/"), "{control}");

    let kept = probe_repo();
    kept.write(".glia/.gitignore", "# a repo's own rules\ncustom/\n");
    import(&kept.top, kept.info(), probe_documents(), opts()).expect("import");
    assert_eq!(std::fs::read(kept.top.join(".glia/.gitignore")).unwrap(), b"# a repo's own rules\ncustom/\n");
    assert!(scip_dir(&kept.top).join(".gitignore").is_file(), "the dir still ignores itself");

    // A second import never rewrites the dir's own ignore file.
    std::fs::write(scip_dir(&kept.top).join(".gitignore"), "*\n# edited\n").unwrap();
    import(&kept.top, kept.info(), probe_documents(), opts()).expect("re-import");
    assert_eq!(std::fs::read_to_string(scip_dir(&kept.top).join(".gitignore")).unwrap(), "*\n# edited\n");
}

#[test]
fn project_root_rebases_document_paths() {
    let repo = Repo::new();
    repo.write("my svc/repos.py", REPOS_PY);
    let canonical = std::fs::canonicalize(&repo.top).unwrap();
    let def = || vec![occ(1, 8, 12, USER_SAVE, DEFINITION)];

    // An index rooted at a subdirectory (percent-encoded): its paths rebase onto it.
    let info = ScipIndexInfo { project_root: format!("file://{}/my%20svc", canonical.display()), ..repo.info() };
    import(&repo.top, info, vec![doc("repos.py", PositionEncoding::Utf8, def())], opts()).expect("import");
    let snap = snapshot(&repo.top);
    assert_eq!(snap.documents[0].path, "my svc/repos.py");
    assert_eq!(snap.meta.project_root, "my svc");

    // An index rooted outside the repo reads its paths as repo-relative.
    let info = ScipIndexInfo { project_root: "file:///work/elsewhere".into(), ..repo.info() };
    import(&repo.top, info, vec![doc("my svc/repos.py", PositionEncoding::Utf8, def())], opts()).expect("import");
    let snap = snapshot(&repo.top);
    assert_eq!((snap.documents[0].path.as_str(), snap.meta.project_root.as_str()), ("my svc/repos.py", ""));

    // --prefix wins over the index's root; `.` is the repo itself.
    let mut prefix = opts();
    prefix.prefix = Some("my svc".into());
    import(&repo.top, repo.info(), vec![doc("repos.py", PositionEncoding::Utf8, def())], prefix).expect("import");
    assert_eq!(snapshot(&repo.top).documents[0].path, "my svc/repos.py");
    let mut dot = opts();
    dot.prefix = Some(".".into());
    let info = ScipIndexInfo { project_root: "file:///work/svc-probe".into(), ..repo.info() };
    import(&repo.top, info, vec![doc("my svc/repos.py", PositionEncoding::Utf8, def())], dot).expect("import");
    assert_eq!(snapshot(&repo.top).meta.project_root, "");

    for bad in ["../elsewhere", "/abs", "a/../../b"] {
        let mut o = opts();
        o.prefix = Some(bad.into());
        assert!(ScipImporter::new(&repo.top, repo.info(), o).is_err(), "--prefix {bad} is refused");
    }
    assert!(ScipImporter::new(&repo.top.join("nope"), repo.info(), opts()).is_err(), "no repo dir");
}

#[test]
fn utf16_ranges_read_names_and_calls() {
    let repo = Repo::new();
    let source = "/* é */ function f🚀() {}\n  f🚀(1);\r\n  g(f🚀);\n";
    repo.write("web/app.ts", source);
    let lines: Vec<&str> = source.split('\n').collect();
    let units = |line: &str, byte: usize| line[..byte].encode_utf16().count() as i32;
    let name_at = |line: &str| {
        let start = line.find("f🚀").unwrap();
        (units(line, start), units(line, start + "f🚀".len()))
    };
    let f = "scip-typescript npm app 1.0 web/`app.ts`/`f🚀`().";
    let (d0, d1) = name_at(lines[0]);
    let (c0, c1) = name_at(lines[1]);
    let (u0, u1) = name_at(lines[2]);
    let occurrences = |enc_bump: i32| {
        vec![
            occ(0, d0, d1, f, DEFINITION),
            occ(1, c0, c1, f, READ),
            occ(2, u0, u1, f, READ),
            occ(1, c0 + enc_bump, c1, f, READ), // inside the surrogate pair when bumped
            occ(9, 0, 1, f, READ),              // no such line
            occ(1, c1, c0, f, READ),            // ends before it starts
        ]
    };
    let ts = |enc| ScipDocumentIn {
        language: "typescript".into(),
        ..doc("web/app.ts", enc, occurrences(2))
    };
    let summary = import(&repo.top, repo.info(), vec![ts(PositionEncoding::Utf16)], opts()).expect("import");
    let snap = snapshot(&repo.top);
    let d = &snap.documents[0];
    assert_eq!(d.defs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["f🚀"]);
    let refs: Vec<(u32, bool)> = d.refs.iter().map(|r| (r.line, r.call)).collect();
    assert_eq!(refs, [(1, true), (2, false)], "`f🚀(1)` is a call, `g(f🚀)` is not");
    assert_eq!((summary.bad_ranges, summary.encoding_unspecified), (3, 0));
    let utf16 = repo.snapshot_bytes();

    // Unspecified reads as UTF-16 and says so.
    let summary = import(&repo.top, repo.info(), vec![ts(PositionEncoding::Unspecified)], opts()).expect("import");
    assert_eq!((summary.bad_ranges, summary.encoding_unspecified), (3, 1));
    assert_eq!(repo.snapshot_bytes(), utf16);

    // The same ranges misread as UTF-8 byte offsets land inside the rocket:
    // every one is a bad range, never a slice mid-char or a wrong name, so
    // the import keeps nothing, fails, and the earlier snapshot stands.
    let err = import(&repo.top, repo.info(), vec![ts(PositionEncoding::Utf8)], opts());
    assert!(err.is_err(), "{err:?}");
    assert_eq!(repo.snapshot_bytes(), utf16);
}

#[test]
fn nothing_is_written_without_a_kept_document() {
    let repo = probe_repo();
    let mut importer = ScipImporter::new(&repo.top, repo.info(), opts()).unwrap();
    for d in probe_documents() {
        importer.document(d);
    }
    drop(importer);
    assert!(!repo.top.join(".glia").exists(), "no finish, nothing written");

    let err = import(&repo.top, repo.info(), vec![doc("/etc/hosts", PositionEncoding::Utf8, vec![])], opts());
    assert!(err.is_err(), "no kept document is an error");
    assert!(!scip_dir(&repo.top).exists());
}
