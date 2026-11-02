//! The build-target snapshot (0.5.7, HT.3): what `bazel query` said about a
//! repo's bazel workspaces, normalised to one [`TargetRow`] per target and
//! written under `<repo>/.glia/targets-snapshot/` by the snapshot step
//! (`glia_snapshots::targets_sync`). A build never runs bazel: the build-target
//! graph is rebuilt from this directory alone.
//!
//! The directory holds [`TARGETS_FILE`] (one JSON row per target, sorted by
//! (workspace, label)) and [`META_FILE`], written LAST, whose `data_hash`
//! covers the rows: a write that stopped part-way leaves no `meta.json` (or
//! one whose hash disagrees), which [`read_targets`] rejects. A repo may hold
//! several bazel workspaces (a monorepo of tutorials holds one `MODULE.bazel`
//! per stage), so a row carries its workspace, the repo-relative directory
//! its labels are relative to (`""` for the repo root), and a sync replaces
//! only its own workspace's rows.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The control directory a snapshot lives under, `<repo>/.glia`. The code
/// domain names the same directory `walk_gating::CONTROL_DIR`; this crate
/// knows no code, so it names it again (the snapshot step's tests pin the two
/// equal).
pub const CONTROL_DIR: &str = ".glia";

/// `<repo>/.glia/targets-snapshot`.
pub const TARGETS_DIR: &str = "targets-snapshot";

/// The rows, one JSON object per line.
pub const TARGETS_FILE: &str = "targets.jsonl";

/// The meta file, written last.
pub const META_FILE: &str = "meta.json";

/// The snapshot encoding this crate writes and reads.
pub const TARGETS_VERSION: u32 = 1;

/// [`TargetRow::kind`] of a rule target (a library, binary, test, alias, ..).
pub const KIND_RULE: &str = "rule";

/// [`TargetRow::kind`] of a source file a package holds (its BUILD file, a
/// file a rule names).
pub const KIND_SOURCE_FILE: &str = "source_file";

/// [`TargetRow::kind`] of a rule's predeclared output.
pub const KIND_GENERATED_FILE: &str = "generated_file";

/// One target of one workspace, as `bazel query` printed it.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRow {
    /// The bazel workspace the label is relative to: a repo-relative
    /// directory, `/`-separated, `""` for the repo root.
    pub workspace: String,
    /// The label as printed (`//main:hello-world`).
    pub label: String,
    /// [`KIND_RULE`], [`KIND_SOURCE_FILE`] or [`KIND_GENERATED_FILE`].
    pub kind: String,
    /// The rule class (`cc_binary`, `go_test`), rules only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_class: Option<String>,
    /// Where the target is declared (a rule's or an output's BUILD file, a
    /// source file itself), repo-relative; `None` outside the repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The 0-based line of [`TargetRow::file`] the target is declared at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Every label-valued attribute the BUILD file set, by attribute name
    /// (`deps`, `data`, `srcs`, `hdrs`, `embed`, `actual`, `exports`,
    /// `runtime_deps`, ..), each list sorted and deduplicated; an empty list
    /// is left out. A `select()` contributes every branch's labels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, Vec<String>>,
    /// The rule's `tags`, sorted and deduplicated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The rule's `testonly` value (a test rule is testonly by default).
    #[serde(default, skip_serializing_if = "is_false")]
    pub testonly: bool,
    /// The target's visibility labels, sorted and deduplicated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visibility: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl TargetRow {
    /// A row with no attributes.
    pub fn new(workspace: impl Into<String>, label: impl Into<String>, kind: &str) -> Self {
        Self {
            workspace: workspace.into(),
            label: label.into(),
            kind: kind.to_string(),
            ..Self::default()
        }
    }

    /// Every label the row's attributes name, in attribute-name order.
    pub fn named_labels(&self) -> impl Iterator<Item = (&str, &str)> {
        self.labels
            .iter()
            .flat_map(|(attr, labels)| labels.iter().map(move |l| (attr.as_str(), l.as_str())))
    }
}

/// What one workspace's last sync recorded.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMeta {
    /// The repo-relative workspace directory (`""` for the repo root).
    pub workspace: String,
    /// `bazel version`'s build label, or `from-file` for a saved query output.
    pub bazel_version: String,
    /// The query expression (`//...:*`).
    pub expr: String,
    /// The rows of this workspace.
    pub rows: usize,
}

impl WorkspaceMeta {
    /// The record of one workspace's sync; `rows` is derived on write.
    pub fn new(
        workspace: impl Into<String>,
        bazel_version: impl Into<String>,
        expr: impl Into<String>,
    ) -> Self {
        Self {
            workspace: workspace.into(),
            bazel_version: bazel_version.into(),
            expr: expr.into(),
            rows: 0,
        }
    }
}

/// `meta.json`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetsMeta {
    /// [`TARGETS_VERSION`].
    pub version: u32,
    /// What wrote it (`glia targets sync`).
    pub generator: String,
    /// One entry per workspace the snapshot holds, sorted by workspace.
    pub workspaces: Vec<WorkspaceMeta>,
    /// Every row, all workspaces.
    pub rows: usize,
    /// xxhash64 (seed 0) of [`TARGETS_FILE`]'s bytes, 16 lowercase hex digits.
    pub data_hash: String,
}

impl TargetsMeta {
    /// A meta at [`TARGETS_VERSION`]; `rows`, each workspace's `rows` and
    /// `data_hash` are derived on write.
    pub fn new(generator: impl Into<String>, workspaces: Vec<WorkspaceMeta>) -> Self {
        Self {
            version: TARGETS_VERSION,
            generator: generator.into(),
            workspaces,
            rows: 0,
            data_hash: String::new(),
        }
    }
}

/// A complete targets snapshot, as [`read_targets`] returns it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetsSnapshot {
    pub meta: TargetsMeta,
    pub rows: Vec<TargetRow>,
}

/// `<root>/.glia/targets-snapshot`.
pub fn targets_dir(root: &Path) -> PathBuf {
    root.join(CONTROL_DIR).join(TARGETS_DIR)
}

/// xxhash64 (seed 0) of `bytes` as 16 lowercase hex digits: the value the
/// code domain's `snapshots::data_hash(&[bytes])` gives. This crate depends
/// on no code crate, so it reaches the one xxhash64 it has, core's: hashing a
/// `RepoId` is one xxhash64 write of the string's bytes. The rows are JSON,
/// so always UTF-8.
pub fn data_hash(bytes: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("{TARGETS_FILE}: {e}"))?;
    Ok(format!(
        "{:016x}",
        glia_core::RepoId::from_canonical(text).0
    ))
}

/// Write the snapshot under `<root>/.glia/targets-snapshot/`: [`TARGETS_FILE`]
/// then [`META_FILE`] LAST. An existing `meta.json` is removed first and each
/// file is written `<file>.tmp` then renamed, so a write that stops part-way
/// leaves a snapshot [`read_targets`] rejects.
///
/// `rows` must be strictly ascending by (workspace, label), every row's
/// workspace one of `meta.workspaces`, which must be strictly ascending, and
/// every row's kind one of the three kinds; anything else is refused before a
/// byte is written. `meta.rows`, each workspace's `rows` and `meta.data_hash`
/// are derived here. Returns the meta as written.
pub fn write_targets(
    root: &Path,
    mut meta: TargetsMeta,
    rows: &[TargetRow],
) -> Result<TargetsMeta, String> {
    if meta.version != TARGETS_VERSION {
        return Err(format!(
            "{META_FILE}: version {} (this writer writes {TARGETS_VERSION})",
            meta.version
        ));
    }
    check_rows(&meta.workspaces, rows)?;
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, row).map_err(|e| format!("{TARGETS_FILE}: {e}"))?;
        bytes.push(b'\n');
    }
    for ws in &mut meta.workspaces {
        ws.rows = rows.iter().filter(|r| r.workspace == ws.workspace).count();
    }
    meta.rows = rows.len();
    meta.data_hash = data_hash(&bytes)?;

    let dir = targets_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let meta_path = dir.join(META_FILE);
    match std::fs::remove_file(&meta_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove {}: {e}", meta_path.display())),
    }
    write_atomic(&dir.join(TARGETS_FILE), &bytes)?;
    let mut meta_bytes =
        serde_json::to_vec_pretty(&meta).map_err(|e| format!("{META_FILE}: {e}"))?;
    meta_bytes.push(b'\n');
    write_atomic(&meta_path, &meta_bytes)?;
    Ok(meta)
}

/// The order and membership rules [`write_targets`] writes and
/// [`read_targets`] checks.
fn check_rows(workspaces: &[WorkspaceMeta], rows: &[TargetRow]) -> Result<(), String> {
    if let Some(w) = workspaces
        .windows(2)
        .find(|w| w[0].workspace.as_bytes() >= w[1].workspace.as_bytes())
    {
        return Err(format!("workspaces are not sorted at {:?}", w[1].workspace));
    }
    for (i, row) in rows.iter().enumerate() {
        if row.label.is_empty() {
            return Err(format!("row {i} has no label"));
        }
        if ![KIND_RULE, KIND_SOURCE_FILE, KIND_GENERATED_FILE].contains(&row.kind.as_str()) {
            return Err(format!("row {:?} has kind {:?}", row.label, row.kind));
        }
        if workspaces
            .binary_search_by(|w| w.workspace.as_bytes().cmp(row.workspace.as_bytes()))
            .is_err()
        {
            return Err(format!(
                "row {:?} names workspace {:?}, which the meta does not list",
                row.label, row.workspace
            ));
        }
        if i > 0 {
            let prev = &rows[i - 1];
            let key = |r: &TargetRow| (r.workspace.clone(), r.label.clone());
            if key(prev) >= key(row) {
                return Err(format!(
                    "rows are not sorted at {:?} {:?}",
                    row.workspace, row.label
                ));
            }
        }
    }
    Ok(())
}

/// The targets snapshot under `<root>/.glia/targets-snapshot/`, or `None`.
///
/// No snapshot directory is the normal "never synced" case: `None`, silently.
/// A directory whose `meta.json` is absent, unreadable or of another version,
/// whose rows do not hash to `meta.data_hash`, do not parse, disagree with the
/// meta's counts or break [`write_targets`]'s order rules is an incomplete
/// snapshot: `None` and one `[targets] snapshot incomplete dir=<d>
/// reason=<r>` line on stderr.
pub fn read_targets(root: &Path) -> Option<TargetsSnapshot> {
    let dir = targets_dir(root);
    if !dir.is_dir() {
        return None;
    }
    match load_targets(&dir) {
        Ok(snapshot) => Some(snapshot),
        Err(reason) => {
            eprintln!(
                "[targets] snapshot incomplete dir={} reason={reason}",
                dir.display()
            );
            None
        }
    }
}

fn load_targets(dir: &Path) -> Result<TargetsSnapshot, String> {
    let meta: TargetsMeta = serde_json::from_slice(&read_required(&dir.join(META_FILE))?)
        .map_err(|e| format!("{META_FILE}: {e}"))?;
    if meta.version != TARGETS_VERSION {
        return Err(format!(
            "{META_FILE}: version {} (reader wants {TARGETS_VERSION})",
            meta.version
        ));
    }
    let bytes = read_required(&dir.join(TARGETS_FILE))?;
    let got = data_hash(&bytes)?;
    if got != meta.data_hash {
        return Err(format!("data_hash {got} != meta {}", meta.data_hash));
    }
    let mut rows = Vec::new();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let row: TargetRow =
            serde_json::from_slice(line).map_err(|e| format!("{TARGETS_FILE}:{}: {e}", i + 1))?;
        rows.push(row);
    }
    check_rows(&meta.workspaces, &rows)?;
    if rows.len() != meta.rows {
        return Err(format!("{} rows, meta says {}", rows.len(), meta.rows));
    }
    for ws in &meta.workspaces {
        let n = rows.iter().filter(|r| r.workspace == ws.workspace).count();
        if n != ws.rows {
            return Err(format!(
                "workspace {:?} holds {n} rows, meta says {}",
                ws.workspace, ws.rows
            ));
        }
    }
    Ok(TargetsSnapshot { meta, rows })
}

fn read_required(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))
}

/// Write `bytes` to `<path>.tmp`, then rename it over `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<TargetRow> {
        let mut lib = TargetRow::new("", "//fortune:fortune", KIND_RULE);
        lib.rule_class = Some("go_library".into());
        lib.file = Some("fortune/BUILD".into());
        lib.line = Some(2);
        lib.labels
            .insert("srcs".into(), vec!["//fortune:fortune.go".into()]);
        lib.visibility = vec!["//visibility:public".into()];
        let src = TargetRow::new("", "//fortune:fortune.go", KIND_SOURCE_FILE);
        let other = TargetRow::new("b", "//x:y", KIND_RULE);
        vec![lib, src, other]
    }

    fn meta() -> TargetsMeta {
        TargetsMeta::new(
            "glia targets sync",
            vec![
                WorkspaceMeta::new("", "7.4.0", "//...:*"),
                WorkspaceMeta::new("b", "from-file", "//...:*"),
            ],
        )
    }

    #[test]
    fn write_then_read_round_trips_with_meta_last() {
        let dir = std::env::temp_dir().join(format!("glia-targets-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let written = write_targets(&dir, meta(), &rows()).expect("write");
        assert_eq!(written.rows, 3);
        assert_eq!(written.workspaces[0].rows, 2);
        assert_eq!(written.workspaces[1].rows, 1);
        let back = read_targets(&dir).expect("read");
        assert_eq!(back.rows, rows());
        assert_eq!(back.meta, written);
        // Without meta.json the snapshot is incomplete, not empty.
        std::fs::remove_file(targets_dir(&dir).join(META_FILE)).expect("rm meta");
        assert!(read_targets(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsorted_or_unlisted_rows_are_refused_before_writing() {
        let dir = std::env::temp_dir().join(format!("glia-targets-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut r = rows();
        r.swap(0, 1);
        assert!(write_targets(&dir, meta(), &r).is_err());
        let mut lone = meta();
        lone.workspaces.pop();
        let err = write_targets(&dir, lone, &rows()).expect_err("unlisted workspace");
        assert!(err.contains("does not list"), "{err}");
        assert!(!dir.exists());
    }

    #[test]
    fn a_row_serialises_without_empty_fields() {
        let src = TargetRow::new("", "//main:BUILD", KIND_SOURCE_FILE);
        assert_eq!(
            serde_json::to_string(&src).expect("json"),
            r#"{"workspace":"","label":"//main:BUILD","kind":"source_file"}"#
        );
    }
}
