//! One integration-test binary for glia-graph (FH.18). `autotests = false` in Cargo.toml, and
//! every tests/*.rs not on [`KEEP`] compiles as one module of the `graph_tests` target:
//! this script writes `$OUT_DIR/test_mods.rs` (one `#[path = ".."] mod <file>;` per file,
//! sorted) and tests/all.rs includes it. A new test file joins with no shared-file edit; run
//! one file's tests with `cargo test -p glia-graph --test graph_tests <file>::`. Before
//! FH.18 each file linked its own copy of the crate and its dependencies.

/// Test files that stay their own `[[test]]` target (Cargo.toml lists each, and this script
/// refuses a mismatch): a test that tooling or a doc names by `--test <name>`, or one that
/// changes process-global state (`set_var`, the working directory, a panic hook) its
/// neighbours would see. A merged test that fails only beside others moves here rather than
/// being weakened.
const KEEP: &[&str] = &[];

/// The `[[test]]` target every other tests/*.rs compiles into, and its root file.
const MERGED_TARGET: &str = "graph_tests";
const MERGED_ROOT: &str = "all";

fn die(msg: &str) -> ! {
    println!("cargo:warning=graph build.rs (FH.18): {msg}");
    std::process::exit(1)
}

/// A tests/*.rs stem that can name a module: an ASCII identifier, not a keyword.
fn is_module_name(stem: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod",
        "move", "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait",
        "true", "try", "type", "unsafe", "use", "where", "while", "yield",
    ];
    let mut chars = stem.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !KEYWORDS.contains(&stem)
}

/// The `name = "<n>"` values of Cargo.toml's `[[test]]` tables.
fn declared_tests(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_test = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_test = line == "[[test]]";
            continue;
        }
        if in_test
            && let Some(value) = line.strip_prefix("name")
            && let Some(value) = value.trim_start().strip_prefix('=')
        {
            names.push(value.trim().trim_matches('"').to_owned());
        }
    }
    names
}

/// Write `$OUT_DIR/test_mods.rs`, the module list tests/all.rs includes.
fn write_test_modules() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    // A directory: cargo reruns this script when any file under tests/ changes, so a new
    // tests/*.rs joins the merged target with no shared-file edit.
    println!("cargo:rerun-if-changed=tests");
    let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        die("CARGO_MANIFEST_DIR is unset")
    };
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        die("OUT_DIR is unset")
    };
    let manifest_dir = std::path::PathBuf::from(manifest_dir);
    let tests_dir = manifest_dir.join("tests");

    // Every kept file needs its own [[test]], and the merged target its one: a test file in
    // neither would silently never run under `autotests = false`.
    let manifest = std::fs::read_to_string(manifest_dir.join("Cargo.toml"))
        .unwrap_or_else(|e| die(&format!("cannot read Cargo.toml: {e}")));
    let declared = declared_tests(&manifest);
    for name in KEEP.iter().copied().chain([MERGED_TARGET]) {
        if !declared.iter().any(|d| d == name) {
            die(&format!("Cargo.toml declares no [[test]] named {name:?}"))
        }
    }
    for name in &declared {
        if name != MERGED_TARGET && !KEEP.contains(&name.as_str()) {
            die(&format!(
                "[[test]] {name:?} is not on build.rs's KEEP list: add it there or delete the table"
            ))
        }
    }

    // A source package without tests/ (an sdist) still builds: the module list is empty.
    // `shared` are helper directories (tests/<dir>/mod.rs, e.g. engine's git_fixture): each is
    // declared once at the target's root, and a test file reaches it with `use crate::<dir>;`
    // (a second `mod <dir>;` would load one file twice, clippy's duplicate_mod).
    let mut merged: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut shared: Vec<(String, std::path::PathBuf)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&tests_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let module = path.join("mod.rs");
                if let Some(name) = path.file_name().and_then(|s| s.to_str())
                    && module.is_file()
                {
                    if !is_module_name(name) {
                        die(&format!(
                            "tests/{name}/mod.rs cannot be a module name: rename it"
                        ))
                    }
                    shared.push((name.to_owned(), module));
                }
                continue;
            }
            if !path.is_file() || path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
                die(&format!("{} has a non-UTF-8 name", path.display()))
            };
            if stem == MERGED_ROOT || KEEP.contains(&stem.as_str()) {
                continue;
            }
            if !is_module_name(&stem) {
                die(&format!(
                    "tests/{stem}.rs cannot be a module name: rename it, or keep it as its own [[test]]"
                ))
            }
            merged.push((stem, path));
        }
    }
    merged.sort();
    shared.sort();
    if let Some((name, _)) = shared
        .iter()
        .find(|(d, _)| merged.iter().any(|(m, _)| m == d))
    {
        die(&format!(
            "tests/{name}.rs and tests/{name}/mod.rs would be one module: rename one"
        ))
    }

    // The first line is FH.18's marker: `grep -h '^// \[tests\]' target/*/build/*/out/test_mods.rs`.
    let mut text = format!(
        "// [tests] crate=graph merged={} kept={} shared={}\n// @generated by build.rs (FH.18): the shared helper directories, then one module per merged tests/*.rs, each sorted.\n",
        merged.len(),
        KEEP.len(),
        shared.len()
    );
    for (stem, path) in shared.iter().chain(&merged) {
        let Some(path) = path.to_str() else {
            die(&format!("{} is not UTF-8", path.display()))
        };
        text.push_str(&format!("#[path = {path:?}]\nmod {stem};\n"));
    }
    let out = std::path::Path::new(&out_dir).join("test_mods.rs");
    // Rewrite only on a change, so an unchanged list never touches the merged target's input.
    if std::fs::read_to_string(&out).ok().as_deref() != Some(text.as_str())
        && let Err(e) = std::fs::write(&out, &text)
    {
        die(&format!("cannot write {}: {e}", out.display()))
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    write_test_modules();
}
