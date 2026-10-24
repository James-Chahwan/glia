//! Compute `GLIA_PARSER_STAMP`: a 64-bit content hash of every source file in
//! the workspace that can change what ends up in a `.gmap`.
//!
//! Why a content hash rather than a hand-bumped constant: the parse cache used
//! to key on the release version alone, so a parser fix merged without a
//! workspace version bump was invisible to every incremental consumer — the
//! cache kept serving pre-fix `FileParse`s while a cold `bench/substrate-gap`
//! run reported the cell fixed. A content hash needs no bump discipline, is
//! exact, and is reproducible across machines and checkouts.
//!
//! Over-invalidation is cheap on purpose: `.gmap` is write-once /
//! rebuild-whole-file, so a false invalidation costs exactly one full rebuild.
//!
//! One commit is one stamp however it is checked out or packaged (ruling
//! STAMP-LOCK, F0.6): inputs are chosen on the workspace-relative key, never
//! the absolute path, and `Cargo.lock` counts only through the hashed crates'
//! dependency closure.
//!
//! `stamp/tests/stamp_inputs.rs` compiles this file as a module, so its helpers
//! are `pub` and `main` is too.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Component, Path, PathBuf};

/// Crates whose source can change node/edge/cell content or `FileParse` shape.
///
/// `activation` is hashed since LD.13: besides the query-time PPR it holds the
/// build-pass registry (`activation::passes`) that orders every resolver and
/// post-pass of a build, and so the stored cross edges and cells.
///
/// Deliberately NOT `py`, `cli` or `doc-sources`: they are transport and
/// orchestration wrappers that cannot change stored bytes, and including `cli`
/// would invalidate every user's parse cache on a CLI tweak. Deliberately NOT
/// `projection-text`: a query-time projection over an already-built graph,
/// not an input to it. The build-target domain crate is hashed since 0.5.4 (F0.2): it shapes
/// `targets.gmap`. Deliberately NOT `embed`: embeddings are query-time and never
/// a build input.
const HASHED_ROOTS: &[&str] = &[
    "core",
    "code-domain",
    "graph",
    "engine",
    "store",
    "parsers",
    "activation",
    "build-domain",
];

/// The lock file. Its bytes are never hashed (ruling STAMP-LOCK, F0.6): what is
/// hashed is the dependency closure of the HASHED_ROOTS crates read from it
/// ([`lock_closure`]), so a packaging that trims workspace members (`maturin
/// sdist` of glia-cli drops glia-py, glia-toy-domain and their pyo3 /
/// inventory / target-lexicon packages) keeps the stamp, while a tree-sitter
/// GRAMMAR bump, which changes parse results with zero `.rs` diff, still moves it.
pub const LOCK_FILE: &str = "Cargo.lock";

/// Floor below which the walk is assumed broken. A silently-empty hash would be
/// the exact hazard this crate exists to kill, so falling short fails the build.
const MIN_INPUTS: usize = 30;

fn die(msg: &str) -> ! {
    println!("cargo:warning=glia-stamp: {msg}");
    println!("cargo:warning=glia-stamp: refusing to emit a stamp that cannot invalidate caches");
    std::process::exit(1)
}

/// FNV-1a (64-bit), inlined so this crate needs no build-dependency.
fn fnv1a(state: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *state ^= u64::from(*b);
        *state = state.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Workspace-root-relative path rendered with `/` separators, built from
/// `Path::components()` rather than `Display` so a Windows checkout produces the
/// same key (and therefore the same stamp) as a Unix one.
pub fn rel_key(ws_root: &Path, p: &Path) -> Option<String> {
    let rel = p.strip_prefix(ws_root).ok()?;
    let mut parts: Vec<&str> = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Every `Cargo.toml`, plus every `.rs` living under a `src` component of the
/// workspace-relative `key` ([`rel_key`]'s output). Never the absolute path: a
/// tree checked out under any directory named `src` (uv unpacks sdists into
/// `.../sdists-v9/.../src`) would otherwise hash `tests/`, `examples/` and
/// `benches/` too.
pub fn is_input(key: &str) -> bool {
    let mut parts: Vec<&str> = key.split('/').collect();
    let Some(file) = parts.pop() else {
        return false;
    };
    if file == "Cargo.toml" {
        return true;
    }
    file.ends_with(".rs") && parts.contains(&"src")
}

fn visit(
    dir: &Path,
    ws_root: &Path,
    files: &mut BTreeMap<String, PathBuf>,
    dirs: &mut Vec<PathBuf>,
) {
    dirs.push(dir.to_path_buf());
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    // `read_dir` order is filesystem-dependent; the BTreeMap key sort is what
    // makes the final hash deterministic, and `dirs` only feeds rerun-if-changed.
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            // Never descend into build output.
            if entry.file_name() == "target" {
                continue;
            }
            visit(&path, ws_root, files, dirs);
        } else if ft.is_file()
            && let Some(key) = rel_key(ws_root, &path)
            && is_input(&key)
        {
            files.insert(key, path);
        }
    }
}

/// What one stamp is computed from.
pub struct Inputs {
    /// Hashed source files by workspace-relative key.
    pub files: BTreeMap<String, PathBuf>,
    /// Every directory walked (rerun-if-changed only, never hashed).
    pub dirs: Vec<PathBuf>,
    /// `Cargo.lock` and its closure text ([`lock_closure`]); `None` when the
    /// tree holds no lock.
    pub lock: Option<(PathBuf, String)>,
}

/// The `[package]` table's `name` of one manifest, if it has one.
pub fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "name"
        {
            return toml_string(value.trim());
        }
    }
    None
}

/// A basic TOML string `"..."` with no escapes (all cargo writes in a lock).
fn toml_string(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains('"') && !inner.contains('\\')).then(|| inner.to_string())
}

#[derive(Default)]
struct LockPackage {
    name: Option<String>,
    version: Option<String>,
    source: Option<String>,
    checksum: Option<String>,
    dependencies: Vec<String>,
}

/// Read a lock's `[[package]]` tables. Anything else but comments, blank lines
/// and the top-level `version = N` is refused, never skipped.
fn parse_lock(lock: &str) -> Result<Vec<LockPackage>, String> {
    let mut packages: Vec<LockPackage> = Vec::new();
    let mut lines = lock.lines().enumerate();
    while let Some((n, raw)) = lines.next() {
        let line = raw.trim();
        let at = n + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[package]]" {
            packages.push(LockPackage::default());
            continue;
        }
        if line.starts_with('[') {
            return Err(format!("{LOCK_FILE}:{at}: unexpected table {line}"));
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("{LOCK_FILE}:{at}: not a key = value line: {line}"));
        };
        let (key, value) = (key.trim(), value.trim());
        let Some(pkg) = packages.last_mut() else {
            if key == "version" && value.parse::<u32>().is_ok() {
                continue;
            }
            return Err(format!("{LOCK_FILE}:{at}: unexpected top-level key {key}"));
        };
        if key == "dependencies" {
            let mut items: Vec<String> = Vec::new();
            if let Some(rest) = value.strip_prefix('[') {
                let mut body = rest.to_string();
                while !body.trim_end().ends_with(']') {
                    let Some((_, next)) = lines.next() else {
                        return Err(format!("{LOCK_FILE}:{at}: unterminated dependencies"));
                    };
                    body.push('\n');
                    body.push_str(next);
                }
                let body = body.trim_end();
                let body = &body[..body.len() - 1];
                for item in body.split([',', '\n']) {
                    let item = item.trim();
                    if item.is_empty() {
                        continue;
                    }
                    let Some(dep) = toml_string(item) else {
                        return Err(format!("{LOCK_FILE}:{at}: bad dependency entry {item}"));
                    };
                    items.push(dep);
                }
            } else {
                return Err(format!("{LOCK_FILE}:{at}: dependencies is not an array"));
            }
            pkg.dependencies = items;
            continue;
        }
        let Some(text) = toml_string(value) else {
            return Err(format!(
                "{LOCK_FILE}:{at}: {key} is not a plain string: {value}"
            ));
        };
        let slot = match key {
            "name" => &mut pkg.name,
            "version" => &mut pkg.version,
            "source" => &mut pkg.source,
            "checksum" => &mut pkg.checksum,
            _ => return Err(format!("{LOCK_FILE}:{at}: unexpected package key {key}")),
        };
        if slot.replace(text).is_some() {
            return Err(format!("{LOCK_FILE}:{at}: {key} given twice"));
        }
    }
    for (i, pkg) in packages.iter().enumerate() {
        if pkg.name.is_none() || pkg.version.is_none() {
            return Err(format!(
                "{LOCK_FILE}: package #{} has no name or version",
                i + 1
            ));
        }
    }
    Ok(packages)
}

/// The one package a lock `dependencies` entry names: `name`, `name version`
/// or `name version (source)`.
fn resolve_dep(packages: &[LockPackage], entry: &str) -> Result<usize, String> {
    let mut parts = entry.splitn(3, ' ');
    let name = parts.next().unwrap_or_default();
    let version = parts.next();
    let source = match parts.next() {
        Some(s) => match s.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            Some(s) => Some(s),
            None => return Err(format!("{LOCK_FILE}: bad dependency source in {entry:?}")),
        },
        None => None,
    };
    let hits: Vec<usize> = packages
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            p.name.as_deref() == Some(name)
                && version.is_none_or(|v| p.version.as_deref() == Some(v))
                && source.is_none_or(|s| p.source.as_deref() == Some(s))
        })
        .map(|(i, _)| i)
        .collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!(
            "{LOCK_FILE}: dependency {entry:?} names no locked package"
        )),
        _ => Err(format!(
            "{LOCK_FILE}: dependency {entry:?} names {} packages",
            hits.len()
        )),
    }
}

fn render(pkg: &LockPackage) -> String {
    format!(
        "{} {}",
        pkg.name.as_deref().unwrap_or_default(),
        pkg.version.as_deref().unwrap_or_default()
    )
}

/// The canonical text of a lock's dependency closure from the `seeds` package
/// names: one line per closure package, `name version source checksum deps=[..]`
/// (`-` for an absent field, each dependency as its resolved `name version`, so
/// cargo shortening `"getrandom 0.3.4"` to `"getrandom"` once a second version
/// leaves the lock changes nothing), sorted. Packages outside the closure are
/// never read into it.
pub fn lock_closure(lock: &str, seeds: &[String]) -> Result<String, String> {
    let packages = parse_lock(lock)?;
    let mut stack: Vec<usize> = Vec::new();
    for seed in seeds {
        let named: Vec<usize> = packages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.name.as_deref() == Some(seed.as_str()))
            .map(|(i, _)| i)
            .collect();
        // A workspace member is the path package (no source) of its name.
        let members: Vec<usize> = named
            .iter()
            .copied()
            .filter(|&i| packages[i].source.is_none())
            .collect();
        let pick = if named.len() == 1 { &named } else { &members };
        match pick.as_slice() {
            [one] => stack.push(*one),
            _ => {
                return Err(format!(
                    "{LOCK_FILE}: hashed crate {seed:?} is not one locked workspace package"
                ));
            }
        }
    }
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    let mut lines: Vec<String> = Vec::new();
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        let pkg = &packages[i];
        let mut deps: Vec<String> = Vec::new();
        for entry in &pkg.dependencies {
            let d = resolve_dep(&packages, entry)?;
            deps.push(render(&packages[d]));
            stack.push(d);
        }
        deps.sort();
        lines.push(format!(
            "{} {} {} deps=[{}]",
            render(pkg),
            pkg.source.as_deref().unwrap_or("-"),
            pkg.checksum.as_deref().unwrap_or("-"),
            deps.join(",")
        ));
    }
    lines.sort();
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

/// Walk the hashed roots of `ws_root` and read its lock closure.
pub fn collect(ws_root: &Path) -> Result<Inputs, String> {
    let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut seeds: Vec<String> = Vec::new();
    for root in HASHED_ROOTS {
        let dir = ws_root.join(root);
        if !dir.is_dir() {
            return Err(format!(
                "hashed root {root:?} is missing under {}",
                ws_root.display()
            ));
        }
        visit(&dir, ws_root, &mut files, &mut dirs);
        // A root without a manifest (`parsers`) seeds nothing itself: its
        // crates are reached as dependencies of the ones that do.
        if let Ok(text) = std::fs::read_to_string(dir.join("Cargo.toml"))
            && let Some(name) = package_name(&text)
        {
            seeds.push(name);
        }
    }
    let lock_path = ws_root.join(LOCK_FILE);
    let lock = if lock_path.is_file() {
        let Ok(text) = std::fs::read_to_string(&lock_path) else {
            return Err(format!("could not read {}", lock_path.display()));
        };
        let text = text.replace('\r', "");
        Some((lock_path, lock_closure(&text, &seeds)?))
    } else {
        None
    };
    Ok(Inputs { files, dirs, lock })
}

/// The stamp: FNV-1a over each input's key, a NUL, its length and its bytes
/// (CR stripped), in key order; the lock contributes its closure text under
/// the key `Cargo.lock`.
pub fn stamp(inputs: &Inputs) -> Result<u64, String> {
    let mut hashed: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
    for (key, path) in &inputs.files {
        let Ok(mut bytes) = std::fs::read(path) else {
            return Err(format!("could not read hashed input {}", path.display()));
        };
        // A CRLF checkout must hash identically to an LF one.
        bytes.retain(|b| *b != b'\r');
        hashed.insert(key, bytes);
    }
    if let Some((_, closure)) = &inputs.lock {
        hashed.insert(LOCK_FILE, closure.as_bytes().to_vec());
    }
    // BTreeMap iterates in key order, so the stamp is independent of readdir order.
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for (key, bytes) in &hashed {
        fnv1a(&mut state, key.as_bytes());
        fnv1a(&mut state, &[0]);
        // Length framing: without it "ab" + "c" and "a" + "bc" collide.
        fnv1a(&mut state, &(bytes.len() as u64).to_le_bytes());
        fnv1a(&mut state, bytes);
    }
    Ok(state)
}

pub fn main() {
    // Resolve the workspace root from the manifest dir, NEVER from cwd: maturin
    // builds the wheel with `working-directory: py`.
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        die("CARGO_MANIFEST_DIR is not set")
    };
    let manifest_dir = PathBuf::from(manifest_dir);
    let Some(ws_root) = manifest_dir.parent() else {
        die("CARGO_MANIFEST_DIR has no parent — cannot locate the workspace root")
    };

    let inputs = match collect(ws_root) {
        Ok(inputs) => inputs,
        Err(e) => die(&e),
    };
    if inputs.lock.is_none() {
        // Not fatal (a packaging may drop it), but never silent: a missing
        // Cargo.lock means a tree-sitter grammar bump stops moving the stamp.
        println!(
            "cargo:warning=glia-stamp: {LOCK_FILE} not found — grammar bumps will not move the stamp"
        );
    }
    if inputs.files.len() < MIN_INPUTS {
        die(&format!(
            "only {} hashed inputs found under {} (expected >= {MIN_INPUTS})",
            inputs.files.len(),
            ws_root.display()
        ));
    }
    let state = match stamp(&inputs) {
        Ok(state) => state,
        Err(e) => die(&e),
    };

    println!("cargo:rustc-env=GLIA_PARSER_STAMP={state:016x}");

    println!("cargo:rerun-if-changed=build.rs");
    for path in inputs.files.values() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    if let Some((path, _)) = &inputs.lock {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    // Directory mtimes too: that is what changes when a file is ADDED, and
    // without these a brand-new parser source would never retrigger this script.
    for dir in &inputs.dirs {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
}
