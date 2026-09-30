//! SCIP index import (CE.1b): the facts glia keeps from a compiler-grade SCIP
//! index, read against the repo's own source and written to
//! `<repo>/.glia/scip-snapshot/` through `glia_code_domain::snapshots`.
//!
//! The protobuf decoding is the caller's (`glia scip import`, CE.1c): it fills
//! the plain input types here one document at a time, so this module has no
//! protobuf dependency and holds one document plus the rows it kept.
//!
//! Three things need the source text and so happen here, never at build:
//! - the NAME at every definition (the build binds a definition by position,
//!   checked by that name). SCIP ranges count characters in the document's
//!   [`PositionEncoding`] (UTF-8 bytes, UTF-16 or UTF-32 code units), so every
//!   offset goes through [`unit_offset_to_byte`], which returns char
//!   boundaries only: a `str` is never sliced by a unit count;
//! - the CALL flag at every reference: SCIP has no call role, so a reference
//!   is a call when [`next_is_call`] finds `(` after its range;
//! - the source hash of every document, so the build can skip a file edited
//!   since the index.
//!
//! Document-local symbols (`local N`) and forward definitions (a prototype is
//! neither the definition nor a use) are dropped. A document path is rebased
//! onto the repo and read only when it stays inside the repo, symlinks
//! included. Nothing is written until [`ScipImporter::finish`].

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use glia_code_domain::snapshots::{
    ScipDefRow, ScipDocumentRecord, ScipMeta, ScipRefRow, ScipSymbolRecord, scip_dir, source_hash, write_scip,
};
use serde::Serialize;

use crate::{ensure_glia_gitignore, repo_label};

/// Largest source file read: a larger one is skipped.
const MAX_SOURCE_BYTES: u64 = 8 << 20;
/// Longest generic argument list, `<` through `>`, [`next_is_call`] looks across.
const MAX_GENERIC_BYTES: usize = 256;
/// `<repo>/.glia/scip-snapshot/.gitignore`: the snapshot dir ignores itself,
/// so a repo whose own `.glia/.gitignore` predates it never commits it.
const SCIP_DIR_GITIGNORE: &str = "# written by glia scip import - regenerable\n*\n";

/// `SymbolRole.Definition`.
const ROLE_DEFINITION: i32 = 0x1;
/// `SymbolRole.Import`.
const ROLE_IMPORT: i32 = 0x2;
/// `SymbolRole.WriteAccess`.
const ROLE_WRITE: i32 = 0x4;
/// `SymbolRole.ForwardDefinition`.
const ROLE_FORWARD: i32 = 0x40;

/// The index's `Metadata`, as the decoder read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScipIndexInfo {
    /// `tool_info.name`, verbatim.
    pub tool: String,
    /// `tool_info.version`, verbatim.
    pub tool_version: String,
    /// `project_root`: the `file://` URI the index's document paths are
    /// relative to, as the index gives it.
    pub project_root: String,
}

/// A document's `position_encoding`: what a range's character offsets count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PositionEncoding {
    /// Not set; read as UTF-16, the LSP default, and counted in
    /// [`ScipImportSummary::encoding_unspecified`].
    #[default]
    Unspecified,
    /// UTF-8 code units (bytes).
    Utf8,
    /// UTF-16 code units.
    Utf16,
    /// UTF-32 code units (chars).
    Utf32,
}

impl PositionEncoding {
    /// The proto enum value: 1 UTF-8, 2 UTF-16, 3 UTF-32; anything else is
    /// [`PositionEncoding::Unspecified`].
    pub fn from_proto(value: i32) -> Self {
        match value {
            1 => Self::Utf8,
            2 => Self::Utf16,
            3 => Self::Utf32,
            _ => Self::Unspecified,
        }
    }

    /// Code units `ch` counts for.
    fn width(self, ch: char) -> usize {
        match self {
            Self::Utf8 => ch.len_utf8(),
            Self::Utf16 | Self::Unspecified => ch.len_utf16(),
            Self::Utf32 => 1,
        }
    }
}

/// One `Occurrence`: its range (0-based lines, character offsets in the
/// document's encoding, end exclusive), symbol and `symbol_roles` bitset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScipOccurrenceIn {
    pub start_line: i32,
    pub start_char: i32,
    pub end_line: i32,
    pub end_char: i32,
    pub symbol: String,
    pub roles: i32,
}

/// One `SymbolInformation`: the symbol and the targets of its
/// `is_implementation` relationships.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScipSymbolIn {
    pub symbol: String,
    pub implements: Vec<String>,
}

/// One `Document` of the index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScipDocumentIn {
    /// Relative to the index's `project_root`, `/`-separated.
    pub relative_path: String,
    pub language: String,
    pub encoding: PositionEncoding,
    pub occurrences: Vec<ScipOccurrenceIn>,
    pub symbols: Vec<ScipSymbolIn>,
}

/// How [`ScipImporter`] places the index's documents and names its caller.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScipImportOptions {
    /// The index's document base, relative to the repo (`--prefix`); overrides
    /// the index's `project_root`. No `..`, not absolute; `.` is the repo.
    pub prefix: Option<String>,
    /// Which surface called the import (`lib`, `cli`); the marker names it.
    pub surface: &'static str,
}

impl Default for ScipImportOptions {
    fn default() -> Self {
        Self { prefix: None, surface: "lib" }
    }
}

/// What an import kept and dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ScipImportSummary {
    /// Documents written.
    pub documents: usize,
    /// Index documents not written: outside the repo, unreadable, not UTF-8,
    /// over 8 MiB, or with no kept row.
    pub skipped: usize,
    /// Definition rows written.
    pub defs: usize,
    /// Reference rows written.
    pub refs: usize,
    /// Of those, references followed by a call paren.
    pub calls: usize,
    /// Symbols written.
    pub symbols: usize,
    /// Occurrences of a document-local (`local N`) or empty symbol, dropped.
    pub locals: usize,
    /// Forward-definition occurrences, dropped.
    pub forward: usize,
    /// Occurrences whose range names a line the file does not have, does not
    /// convert to char boundaries, ends before it starts, or is a definition
    /// that is not a non-empty single-line range; dropped.
    pub bad_ranges: usize,
    /// Documents read with no position encoding (read as UTF-16).
    pub encoding_unspecified: usize,
}

/// One kept document's rows, symbols as interned ids until [`ScipImporter::finish`].
#[derive(Debug)]
struct DocAcc {
    /// The smallest language any input document of this path gave.
    language: String,
    /// The smallest hash any read of this path gave (they differ only when
    /// the file changed mid-import, a race the build's staleness check absorbs).
    source_hash: String,
    /// Index documents that named this path.
    inputs: usize,
    defs: Vec<ScipDefRow>,
    refs: Vec<ScipRefRow>,
}

/// Streams an index's documents into a SCIP snapshot: [`ScipImporter::new`]
/// once, [`ScipImporter::document`] per decoded document, then
/// [`ScipImporter::finish`], the only step that writes.
#[derive(Debug)]
pub struct ScipImporter {
    /// The repo root, canonical.
    repo: PathBuf,
    info: ScipIndexInfo,
    surface: &'static str,
    /// The documents' base relative to the repo: `/`-joined components, `""`
    /// for the repo itself.
    base: Vec<String>,
    /// Symbol string -> interned id (first-seen order; final ids are assigned
    /// in string order by `finish`).
    interned: BTreeMap<String, u32>,
    /// Interned id -> interned ids of the symbols it implements.
    implements: BTreeMap<u32, BTreeSet<u32>>,
    /// Kept documents by repo-relative path.
    documents: BTreeMap<String, DocAcc>,
    /// Index documents skipped before any row was read.
    skipped: usize,
    locals: usize,
    forward: usize,
    bad_ranges: usize,
    encoding_unspecified: usize,
    /// More than `u32::MAX` distinct symbols: `finish` refuses to write.
    overflow: bool,
}

impl ScipImporter {
    /// An importer for the repo at `repo_root`. The documents' base is
    /// `opts.prefix` when given, else the index's `project_root` made relative
    /// to the repo; a root outside the repo or not a `file://` URI reads the
    /// document paths as repo-relative, with one `[scip] warning:` line.
    ///
    /// Errs when the repo is not a directory or the prefix is absolute or has
    /// a `..` component.
    pub fn new(repo_root: &Path, info: ScipIndexInfo, opts: ScipImportOptions) -> Result<Self, String> {
        let repo = std::fs::canonicalize(repo_root).map_err(|e| format!("repo {}: {e}", repo_root.display()))?;
        if !repo.is_dir() {
            return Err(format!("not a directory: {}", repo_root.display()));
        }
        let base = match &opts.prefix {
            Some(prefix) => {
                if is_absolute(prefix) {
                    return Err(format!("--prefix {prefix:?} is absolute; give a directory relative to the repo"));
                }
                components(prefix).ok_or_else(|| format!("--prefix {prefix:?} has a `..` component"))?
            }
            None => match root_under_repo(&repo, &info.project_root) {
                Some(base) => base,
                None => {
                    eprintln!(
                        "[scip] warning: project_root {} is outside {}; document paths read as repo-relative",
                        info.project_root,
                        repo.display()
                    );
                    Vec::new()
                }
            },
        };
        Ok(Self {
            repo,
            info,
            surface: opts.surface,
            base,
            interned: BTreeMap::new(),
            implements: BTreeMap::new(),
            documents: BTreeMap::new(),
            skipped: 0,
            locals: 0,
            forward: 0,
            bad_ranges: 0,
            encoding_unspecified: 0,
            overflow: false,
        })
    }

    /// Read one decoded document against its source and keep its rows. A
    /// document outside the repo (an absolute path, a `..` component, a
    /// symlink out), missing, not a regular file, over 8 MiB or not UTF-8 is
    /// skipped unread. Documents naming the same path merge.
    pub fn document(&mut self, d: ScipDocumentIn) {
        let Some(path) = self.document_path(&d.relative_path) else {
            self.skipped += 1;
            return;
        };
        let Some(bytes) = read_inside(&self.repo, &path) else {
            self.skipped += 1;
            return;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            self.skipped += 1;
            return;
        };
        let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
        if d.encoding == PositionEncoding::Unspecified {
            self.encoding_unspecified += 1;
        }

        let mut defs = Vec::new();
        let mut refs = Vec::new();
        for o in &d.occurrences {
            if o.symbol.is_empty() || is_local(&o.symbol) {
                self.locals += 1;
                continue;
            }
            if o.roles & ROLE_FORWARD != 0 {
                self.forward += 1;
                continue;
            }
            let Some(range) = convert_range(&lines, o, d.encoding) else {
                self.bad_ranges += 1;
                continue;
            };
            if o.roles & ROLE_DEFINITION != 0 {
                let name = (range.start_line == range.end_line)
                    .then(|| lines.get(range.start_line).and_then(|l| l.get(range.start_byte..range.end_byte)))
                    .flatten()
                    .filter(|name| !name.is_empty());
                let Some(name) = name else {
                    self.bad_ranges += 1;
                    continue;
                };
                let Some(s) = self.intern(&o.symbol) else { continue };
                defs.push(ScipDefRow { s, line: range.line, name: name.to_string() });
            } else {
                let call = lines.get(range.end_line).is_some_and(|l| next_is_call(l, range.end_byte));
                let Some(s) = self.intern(&o.symbol) else { continue };
                refs.push(ScipRefRow {
                    s,
                    line: range.line,
                    call,
                    write: o.roles & ROLE_WRITE != 0,
                    import: o.roles & ROLE_IMPORT != 0,
                });
            }
        }
        for sym in &d.symbols {
            self.record_implements(sym);
        }

        let hash = source_hash(&bytes);
        let acc = self.documents.entry(path).or_insert_with(|| DocAcc {
            language: d.language.clone(),
            source_hash: hash.clone(),
            inputs: 0,
            defs: Vec::new(),
            refs: Vec::new(),
        });
        acc.inputs += 1;
        if d.language < acc.language {
            acc.language = d.language;
        }
        if hash < acc.source_hash {
            acc.source_hash = hash;
        }
        acc.defs.extend(defs);
        acc.refs.extend(refs);
    }

    /// Write the snapshot: symbols a kept row names (and, transitively, the
    /// symbols a kept symbol implements) get ids `0..n` in symbol-string
    /// order, so the files never depend on the order the index listed its
    /// documents in; documents sorted by path, rows by their `sort_key`.
    /// Creates `.glia/.gitignore` when absent and the snapshot dir's own
    /// `.gitignore`, then prints the `[scip] import` marker.
    ///
    /// Errs, writing nothing, when no document was kept.
    pub fn finish(self) -> Result<ScipImportSummary, String> {
        if self.overflow {
            return Err(format!("the index names more than {} symbols", u32::MAX));
        }
        let mut skipped = self.skipped;
        let mut kept_docs = Vec::new();
        for (path, acc) in self.documents {
            if acc.defs.is_empty() && acc.refs.is_empty() {
                skipped += acc.inputs;
            } else {
                kept_docs.push((path, acc));
            }
        }
        if kept_docs.is_empty() {
            return Err(format!(
                "no document of the index was kept ({skipped} skipped: outside {}, unreadable, or with no \
                 definition or reference); check the index's project_root or give --prefix",
                self.repo.display()
            ));
        }

        // Keep the symbols rows name, then what they implement, transitively.
        let mut kept = vec![false; self.interned.len()];
        let mut stack: Vec<u32> = Vec::new();
        for (_, acc) in &kept_docs {
            for id in acc.defs.iter().map(|r| r.s).chain(acc.refs.iter().map(|r| r.s)) {
                mark(id, &mut kept, &mut stack);
            }
        }
        while let Some(id) = stack.pop() {
            for &target in self.implements.get(&id).into_iter().flatten() {
                mark(target, &mut kept, &mut stack);
            }
        }

        // Final ids in symbol-string order.
        let mut final_id: Vec<Option<u32>> = vec![None; self.interned.len()];
        let mut symbols: Vec<ScipSymbolRecord> = Vec::new();
        for (symbol, &tmp) in &self.interned {
            if kept.get(tmp as usize).copied().unwrap_or(false) {
                let id = u32::try_from(symbols.len()).map_err(|_| "symbol id overflow".to_string())?;
                if let Some(slot) = final_id.get_mut(tmp as usize) {
                    *slot = Some(id);
                }
                symbols.push(ScipSymbolRecord { id, symbol: symbol.clone(), implements: Vec::new() });
            }
        }
        let remap = |tmp: u32| final_id.get(tmp as usize).copied().flatten();
        for (&tmp, targets) in &self.implements {
            let Some(id) = remap(tmp) else { continue };
            let mut ids: Vec<u32> = targets.iter().filter_map(|&t| remap(t)).filter(|&t| t != id).collect();
            ids.sort_unstable();
            ids.dedup();
            if let Some(record) = symbols.get_mut(id as usize) {
                record.implements = ids;
            }
        }

        let mut documents = Vec::with_capacity(kept_docs.len());
        for (path, acc) in kept_docs {
            let mut defs: Vec<ScipDefRow> =
                acc.defs.into_iter().filter_map(|r| Some(ScipDefRow { s: remap(r.s)?, ..r })).collect();
            let mut refs: Vec<ScipRefRow> =
                acc.refs.into_iter().filter_map(|r| Some(ScipRefRow { s: remap(r.s)?, ..r })).collect();
            defs.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
            refs.sort_by_key(ScipRefRow::sort_key);
            documents.push(ScipDocumentRecord { path, language: acc.language, source_hash: acc.source_hash, defs, refs });
        }

        let meta = ScipMeta::new(self.info.tool.clone(), self.info.tool_version.clone(), self.base.join("/"), skipped);
        write_scip(&self.repo, meta, &documents, &symbols)?;
        ensure_glia_gitignore(&self.repo)?;
        ensure_dir_gitignore(&scip_dir(&self.repo))?;

        let summary = ScipImportSummary {
            documents: documents.len(),
            skipped,
            defs: documents.iter().map(|d| d.defs.len()).sum(),
            refs: documents.iter().map(|d| d.refs.len()).sum(),
            calls: documents.iter().flat_map(|d| &d.refs).filter(|r| r.call).count(),
            symbols: symbols.len(),
            locals: self.locals,
            forward: self.forward,
            bad_ranges: self.bad_ranges,
            encoding_unspecified: self.encoding_unspecified,
        };
        eprintln!("{}", import_marker(&repo_label(&self.repo), &self.info, &summary, self.surface));
        Ok(summary)
    }

    /// `<base>/<relative_path>`, `/`-joined, or `None` when the path is
    /// absolute, has a `..` component or names the base itself.
    fn document_path(&self, relative_path: &str) -> Option<String> {
        if is_absolute(relative_path) {
            return None;
        }
        let mut parts = self.base.clone();
        parts.extend(components(relative_path)?);
        (!parts.is_empty()).then(|| parts.join("/"))
    }

    /// The interned id of `symbol`, or `None` past `u32::MAX` symbols.
    fn intern(&mut self, symbol: &str) -> Option<u32> {
        if let Some(&id) = self.interned.get(symbol) {
            return Some(id);
        }
        let Ok(id) = u32::try_from(self.interned.len()) else {
            self.overflow = true;
            return None;
        };
        self.interned.insert(symbol.to_string(), id);
        Some(id)
    }

    /// Record `sym`'s implementation targets; local and empty symbols carry none.
    fn record_implements(&mut self, sym: &ScipSymbolIn) {
        if sym.symbol.is_empty() || is_local(&sym.symbol) {
            return;
        }
        let targets: Vec<&String> = sym
            .implements
            .iter()
            .filter(|t| !t.is_empty() && !is_local(t) && **t != sym.symbol)
            .collect();
        if targets.is_empty() {
            return;
        }
        let Some(id) = self.intern(&sym.symbol) else { return };
        let mut ids = BTreeSet::new();
        for target in targets {
            let Some(t) = self.intern(target) else { return };
            ids.insert(t);
        }
        self.implements.entry(id).or_default().extend(ids);
    }
}

/// Mark interned symbol `id` kept, queueing it once for its implements.
fn mark(id: u32, kept: &mut [bool], stack: &mut Vec<u32>) {
    if let Some(slot) = kept.get_mut(id as usize)
        && !*slot
    {
        *slot = true;
        stack.push(id);
    }
}

/// A range converted to byte offsets on its lines.
struct ByteRange {
    /// The 0-based start line, as stored.
    line: u32,
    start_line: usize,
    start_byte: usize,
    end_line: usize,
    end_byte: usize,
}

/// `o`'s range as byte offsets, or `None` when a line is missing, an offset
/// does not land on a char boundary, or the range ends before it starts.
fn convert_range(lines: &[&str], o: &ScipOccurrenceIn, enc: PositionEncoding) -> Option<ByteRange> {
    let line = u32::try_from(o.start_line).ok()?;
    let start_line = usize::try_from(o.start_line).ok()?;
    let end_line = usize::try_from(o.end_line).ok()?;
    let start_byte = unit_offset_to_byte(lines.get(start_line)?, o.start_char, enc)?;
    let end_byte = unit_offset_to_byte(lines.get(end_line)?, o.end_char, enc)?;
    if (end_line, end_byte) < (start_line, start_byte) {
        return None;
    }
    Some(ByteRange { line, start_line, start_byte, end_line, end_byte })
}

/// The byte offset in `line` of the char boundary `units` code units in, or
/// `None` when `units` is negative, lands inside a char or passes the line
/// end. [`PositionEncoding::Unspecified`] counts UTF-16 units. Never slices.
pub fn unit_offset_to_byte(line: &str, units: i32, enc: PositionEncoding) -> Option<usize> {
    let target = usize::try_from(units).ok()?;
    let mut at = 0usize;
    for (byte, ch) in line.char_indices() {
        if at == target {
            return Some(byte);
        }
        if at > target {
            return None;
        }
        at += enc.width(ch);
    }
    (at == target).then_some(line.len())
}

/// Whether the reference ending at byte `end_byte` of `line` is a call: past
/// spaces and tabs, an optional generic argument list (`::<..>` or `<..>`,
/// balanced, at most 256 bytes, on this line) and more blanks, the next char
/// is `(`. An `end_byte` off a char boundary or past the line is not a call.
pub fn next_is_call(line: &str, end_byte: usize) -> bool {
    let Some(rest) = line.get(end_byte..) else { return false };
    let mut rest = rest.trim_start_matches([' ', '\t']);
    let generic = rest.strip_prefix("::").unwrap_or(rest);
    if generic.starts_with('<') {
        let Some(len) = balanced_angles(generic) else { return false };
        let Some(after) = generic.get(len..) else { return false };
        rest = after.trim_start_matches([' ', '\t']);
    } else if generic.len() != rest.len() {
        // `::` not followed by `<`: a path (`f::new`), never a call of `f`.
        return false;
    }
    rest.starts_with('(')
}

/// Byte length of the balanced `<...>` that `s` starts with, when it closes
/// within [`MAX_GENERIC_BYTES`]. `<` and `>` are ASCII, so a byte scan never
/// splits a char.
fn balanced_angles(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in s.bytes().enumerate().take(MAX_GENERIC_BYTES) {
        match b {
            b'<' => depth += 1,
            b'>' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// A SCIP document-local symbol (`local <id>`): it has no graph node.
fn is_local(symbol: &str) -> bool {
    symbol.starts_with("local ")
}

/// Absolute on any platform the index could come from: a leading `/` or `\`,
/// or what this platform calls absolute.
fn is_absolute(path: &str) -> bool {
    path.starts_with(['/', '\\']) || Path::new(path).is_absolute()
}

/// `path`'s components split on `/` or `\`, with empty and `.` ones dropped,
/// or `None` when one is `..`.
fn components(path: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            _ => out.push(part.to_string()),
        }
    }
    Some(out)
}

/// The index's `project_root` (a `file://` URI) as components relative to
/// `repo`, or `None` when it does not parse, does not exist or is outside.
fn root_under_repo(repo: &Path, project_root: &str) -> Option<Vec<String>> {
    let rest = project_root.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let decoded = percent_decode(rest)?;
    let path = Path::new(&decoded);
    if !path.is_absolute() {
        return None;
    }
    let canonical = std::fs::canonicalize(path).ok()?;
    let rel = canonical.strip_prefix(repo).ok()?;
    rel.components().map(|c| c.as_os_str().to_str().map(str::to_string)).collect()
}

/// `%XX` escapes decoded; `None` on a malformed escape or non-UTF-8 result.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The bytes of `<repo>/<rel>` when it resolves (symlinks followed) to a
/// regular file inside `repo` of at most [`MAX_SOURCE_BYTES`]; else `None`.
fn read_inside(repo: &Path, rel: &str) -> Option<Vec<u8>> {
    let full = repo.join(rel);
    std::fs::symlink_metadata(&full).ok()?;
    let canonical = std::fs::canonicalize(&full).ok()?;
    if !canonical.starts_with(repo) || canonical == repo {
        return None;
    }
    let meta = std::fs::metadata(&canonical).ok()?;
    if !meta.is_file() || meta.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&canonical).ok()?.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_SOURCE_BYTES).then_some(bytes)
}

/// Create `<dir>/.gitignore` with [`SCIP_DIR_GITIGNORE`] unless one exists.
fn ensure_dir_gitignore(dir: &Path) -> Result<(), String> {
    let path = dir.join(".gitignore");
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => file
            .write_all(SCIP_DIR_GITIGNORE.as_bytes())
            .map_err(|e| format!("write {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(format!("create {}: {e}", path.display())),
    }
}

/// `[scip] import repo=<label> tool=<tool>@<version> documents=N skipped=N
/// defs=N refs=N calls=N symbols=N locals=N forward=N bad_ranges=N
/// encoding_unspecified=N surface=<surface>`; whitespace in the tool name or
/// version becomes `_` so the line stays `key=value` tokens.
fn import_marker(label: &str, info: &ScipIndexInfo, s: &ScipImportSummary, surface: &str) -> String {
    let token = |v: &str| v.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect::<String>();
    format!(
        "[scip] import repo={label} tool={}@{} documents={} skipped={} defs={} refs={} calls={} symbols={} \
         locals={} forward={} bad_ranges={} encoding_unspecified={} surface={surface}",
        token(&info.tool),
        token(&info.tool_version),
        s.documents,
        s.skipped,
        s.defs,
        s.refs,
        s.calls,
        s.symbols,
        s.locals,
        s.forward,
        s.bad_ranges,
        s.encoding_unspecified
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_the_specified_line() {
        let info = ScipIndexInfo {
            tool: "scip-python".into(),
            tool_version: "0.6.0".into(),
            project_root: "file:///work/svc-probe".into(),
        };
        let summary = ScipImportSummary {
            documents: 2,
            defs: 2,
            refs: 1,
            calls: 1,
            symbols: 2,
            ..ScipImportSummary::default()
        };
        assert_eq!(
            import_marker("g1", &info, &summary, "lib"),
            "[scip] import repo=g1 tool=scip-python@0.6.0 documents=2 skipped=0 defs=2 refs=1 calls=1 \
             symbols=2 locals=0 forward=0 bad_ranges=0 encoding_unspecified=0 surface=lib"
        );
        let spaced = ScipIndexInfo { tool: "rust analyzer".into(), tool_version: "".into(), ..info };
        assert!(import_marker("g1", &spaced, &summary, "cli").contains(" tool=rust_analyzer@ documents=2"));
    }

    #[test]
    fn paths_split_and_refuse_parent_components() {
        assert_eq!(components("a/./b//c"), Some(vec!["a".to_string(), "b".into(), "c".into()]));
        assert_eq!(components("a\\b"), Some(vec!["a".to_string(), "b".into()]));
        assert_eq!(components("."), Some(Vec::new()));
        assert_eq!(components("a/../b"), None);
        assert_eq!(components("a\\..\\b"), None);
        assert!(is_absolute("/etc") && is_absolute("\\etc") && !is_absolute("etc"));
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("/a%20b/%C3%A9").as_deref(), Some("/a b/é"));
        assert_eq!(percent_decode("/plain").as_deref(), Some("/plain"));
        assert_eq!(percent_decode("/bad%2"), None);
        assert_eq!(percent_decode("/bad%zz"), None);
        assert_eq!(percent_decode("/bad%FF"), None, "not UTF-8");
    }

    #[test]
    fn unit_offsets_never_panic_on_hostile_lines() {
        let lines = ["", "a", "é", "🚀", "\u{0}\u{10FFFF}x", "a\u{301}b"];
        for line in lines {
            for enc in [
                PositionEncoding::Unspecified,
                PositionEncoding::Utf8,
                PositionEncoding::Utf16,
                PositionEncoding::Utf32,
            ] {
                for units in [i32::MIN, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, i32::MAX] {
                    if let Some(byte) = unit_offset_to_byte(line, units, enc) {
                        assert!(line.is_char_boundary(byte));
                        for end in 0..line.len() + 2 {
                            let _ = next_is_call(line, end);
                        }
                    }
                }
            }
        }
    }
}
