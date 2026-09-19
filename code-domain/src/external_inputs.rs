//! Externally supplied node cells: the `.glia/cells.jsonl` and
//! `.glia/vectors.jsonl` sidecars, their row shapes, the entry rules every
//! writer shares (LF.1a), and the typed reader of CONSTRAINT entries (LF.4a:
//! [`ConstraintRule`], [`parse_constraints`]).
//!
//! LAYOUT. `.glia/` is glia's control dir: the walk never enters it (LF.1d)
//! and readers open their files directly.
//! - Checked in: `.glia/overlay.toml` (LF.2a) and `.glia/cells.jsonl` (this
//!   module) - curated knowledge that belongs to the repo.
//! - Local, gitignored by default: `.glia/vectors.jsonl` (this module),
//!   `.glia/docs-snapshot/`, `.glia/history-snapshot/` (LF.5a) and
//!   `.glia/test-snapshot/` (LF.6a) - regenerable inputs. A `.glia/.gitignore`
//!   lists only these; `overlay.toml` and `cells.jsonl` are never in it, and
//!   the graph layout dir ignores itself (LC.9).
//!
//! Every input file under `.glia/` is fingerprinted into the layout's
//! staleness check (LF.1d), so editing a sidecar makes the next
//! `load_or_rebuild` rebuild. The build is what applies a sidecar: a cell
//! written into a `.gmap` directly would be dropped by the next rebuild.
//!
//! ROWS. One JSON object per non-empty line. A cell row carries one entry:
//! `{"qname":"app::charge","cell":"CONV","entry":{"source":"api","id":"000001","text":"..."}}`,
//! optionally `kind` (a NodeKind NAME, to split a CLASS / SERVICE pair that
//! share a qname) and `hint` (the node's move-stable identity hint,
//! `graph::identity::Identity::hint`, captured by the writer so a moved node
//! still binds). A vector row carries the little-endian bytes of one embedding
//! in standard padded base64: `{"qname":"app::charge","dims":2,"b64":"AACAPwAAAEA="}`,
//! optionally `model`.
//!
//! PAYLOADS. A CONSTRAINT, DECISION or CONV cell is a JSON array of entry
//! objects, one per `(source, id)`, sorted by `(source, id)`, serialized
//! compactly with sorted keys ([`merge_entry`]). A VECTOR cell is
//! `CellPayload::Bytes`. The one resolver and apply function live in the graph
//! crate (`repo_graph_graph::cells`), so the build, a live in-memory write and
//! a persisted write-through bind a row to the same node.
//!
//! CONSTRAINT ENTRIES (the rule schema `glia check`, LE.8, reads through
//! [`parse_constraints`] and nowhere else). The overlay's `[[constraint]]`
//! stanzas (LF.4a) are stored as
//! `{"source":"overlay","id","kind","from"?,"to"?,"from_raw"?,"to_raw"?,"scope"?,"scope_raw"?,"categories"?,"text"?,"origin":"human|llm","decl":".glia/overlay.toml:<line>"}`:
//! `from` / `to` / `scope` hold the scope RESOLVED to a repo-relative path
//! (`.` = the repo root) and the `*_raw` keys what the stanza said (a project
//! label, a qname or a path), so a reader never re-resolves a label. An entry
//! written through the cell API (`source` `api`) carries the scope strings as
//! its writer gave them.

use std::collections::BTreeMap;
use std::io::{self, Write as _};
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use repo_graph_core::{CellPayload, CellTypeId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::cell_type;
use crate::edge_category;
use crate::glia_config::{CONSTRAINT_KINDS, MAX_ID_CHARS, MAX_NOTE_CHARS};

/// Repo-relative path of the checked-in cell sidecar.
pub const CELLS_FILE: &str = ".glia/cells.jsonl";
/// Repo-relative path of the local vector sidecar.
pub const VECTORS_FILE: &str = ".glia/vectors.jsonl";
/// Largest VECTOR payload accepted, in bytes (16384 f32 dims).
pub const MAX_VECTOR_BYTES: usize = 65_536;
/// The cell types an external writer may set. Every other type is owned by
/// an extractor or a build pass.
pub const WRITABLE: &[CellTypeId] =
    &[cell_type::CONSTRAINT, cell_type::DECISION, cell_type::CONV, cell_type::VECTOR];
/// Who wrote an entry: the write API (LF.1b / LF.1c), the overlay's declared
/// knowledge (LF.4a) or an ADR (LF.4b).
pub const ENTRY_SOURCES: &[&str] = &["api", "overlay", "adr"];

/// One `.glia/cells.jsonl` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellRow {
    pub qname: String,
    /// NodeKind NAME (`"SERVICE"`); disambiguates nodes sharing a qname.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Move-stable identity hint captured at write time (LB.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Cell type NAME (`"CONV"`).
    pub cell: String,
    pub entry: Value,
}

/// One `.glia/vectors.jsonl` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorRow {
    pub qname: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// When given, the payload must be exactly `dims * 4` bytes (f32).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dims: Option<u32>,
    /// Standard padded base64 of the payload bytes.
    pub b64: String,
}

/// What one write puts on its node.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum WritePayload {
    /// One entry, upserted by `(source, id)` into the cell's entry array.
    Entry(Value),
    /// The node's whole VECTOR cell.
    Vector { bytes: Vec<u8>, model: Option<String>, dims: Option<u32> },
}

/// One cell write, from a sidecar row or a live API call. Built by
/// [`CellWrite::entry`] / [`CellWrite::vector`] (plus [`CellWrite::with_kind`]
/// / [`CellWrite::with_hint`]) or converted from a row.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CellWrite {
    pub qname: String,
    pub kind: Option<String>,
    pub hint: Option<String>,
    pub cell: CellTypeId,
    pub payload: WritePayload,
}

impl CellWrite {
    /// An entry write of `cell` on `qname`.
    pub fn entry(qname: impl Into<String>, cell: CellTypeId, value: Value) -> Self {
        CellWrite { qname: qname.into(), kind: None, hint: None, cell, payload: WritePayload::Entry(value) }
    }

    /// A VECTOR write on `qname`.
    pub fn vector(
        qname: impl Into<String>,
        bytes: Vec<u8>,
        model: Option<String>,
        dims: Option<u32>,
    ) -> Self {
        CellWrite {
            qname: qname.into(),
            kind: None,
            hint: None,
            cell: cell_type::VECTOR,
            payload: WritePayload::Vector { bytes, model, dims },
        }
    }

    /// Restrict the target to nodes of this NodeKind NAME.
    pub fn with_kind(mut self, kind: Option<String>) -> Self {
        self.kind = kind;
        self
    }

    /// The move-stable hint of the node the writer meant.
    pub fn with_hint(mut self, hint: Option<String>) -> Self {
        self.hint = hint;
        self
    }
}

/// A row whose `cell` names no cell type fails.
impl TryFrom<&CellRow> for CellWrite {
    type Error = String;

    fn try_from(row: &CellRow) -> Result<Self, String> {
        let cell = cell_type_id(&row.cell).ok_or_else(|| format!("unknown cell type {:?}", row.cell))?;
        Ok(CellWrite::entry(row.qname.clone(), cell, row.entry.clone())
            .with_kind(row.kind.clone())
            .with_hint(row.hint.clone()))
    }
}

/// Decodes the base64 and checks the size (and `dims`, when given).
impl TryFrom<&VectorRow> for CellWrite {
    type Error = String;

    fn try_from(row: &VectorRow) -> Result<Self, String> {
        let bytes = b64_decode(&row.b64)?;
        check_vector(&bytes, row.dims)?;
        Ok(CellWrite::vector(row.qname.clone(), bytes, row.model.clone(), row.dims)
            .with_kind(row.kind.clone())
            .with_hint(row.hint.clone()))
    }
}

/// The cell type registered under `name` (`"CONV"`).
pub fn cell_type_id(name: &str) -> Option<CellTypeId> {
    cell_type::ALL.iter().find(|(_, n)| *n == name).map(|(id, _)| *id)
}

/// A VECTOR payload is 1..=[`MAX_VECTOR_BYTES`] bytes and, when `dims` is
/// given, exactly `dims * 4` of them.
pub fn check_vector(bytes: &[u8], dims: Option<u32>) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() > MAX_VECTOR_BYTES {
        return Err(format!("vector is {} bytes; must be 1..={MAX_VECTOR_BYTES}", bytes.len()));
    }
    if let Some(d) = dims {
        let want = usize::try_from(d).ok().and_then(|d| d.checked_mul(4));
        if want != Some(bytes.len()) {
            return Err(format!("vector is {} bytes but dims={d} needs {} (f32)", bytes.len(), u64::from(d) * 4));
        }
    }
    Ok(())
}

/// Standard padded base64.
pub fn b64_encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Inverse of [`b64_encode`].
pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    STANDARD.decode(s.trim()).map_err(|e| format!("b64: {e}"))
}

/// Read a JSONL sidecar: one row per non-empty line, in file order. A line
/// that is not a row becomes `"<file>:<line>: <error>"` and the rest still
/// load; a missing file is empty. Never panics.
pub fn read_rows<T: DeserializeOwned>(path: &Path) -> (Vec<T>, Vec<String>) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (Vec::new(), Vec::new()),
        Err(e) => return (Vec::new(), vec![format!("{}: cannot read: {e}", path.display())]),
    };
    let bytes = bytes.strip_prefix("\u{feff}".as_bytes()).unwrap_or(&bytes);
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        let parsed = std::str::from_utf8(line)
            .map_err(|e| format!("not UTF-8: {e}"))
            .and_then(|l| {
                let l = l.trim();
                if l.is_empty() {
                    Ok(None)
                } else {
                    serde_json::from_str::<T>(l).map(Some).map_err(|e| e.to_string())
                }
            });
        match parsed {
            Ok(Some(row)) => rows.push(row),
            Ok(None) => {}
            Err(e) => errors.push(format!("{}:{}: {e}", path.display(), i + 1)),
        }
    }
    (rows, errors)
}

/// Write a JSONL sidecar: `<file>.tmp`, then a rename, so a reader never sees
/// a half-written file (and LF.1d's fingerprint never counts the `.tmp`).
/// Each row is written compactly with sorted keys ([`canonical`]), in the
/// order given: callers sort.
pub fn write_rows<T: Serialize>(path: &Path, rows: &[T]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let mut out = io::BufWriter::new(std::fs::File::create(&tmp)?);
    for row in rows {
        let v = serde_json::to_value(row).map_err(io::Error::other)?;
        serde_json::to_writer(&mut out, &canonical(&v)).map_err(io::Error::other)?;
        out.write_all(b"\n")?;
    }
    out.into_inner().map_err(io::IntoInnerError::into_error)?.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// `v` with every object's keys in sorted order. serde_json's `Map` keeps
/// insertion order when any crate in the build enables `preserve_order`
/// (feature unification is workspace-wide), so stored payloads rebuild each
/// object through a `BTreeMap`.
pub fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let sorted: BTreeMap<&String, Value> = m.iter().map(|(k, v)| (k, canonical(v))).collect();
            let mut out = Map::new();
            for (k, v) in sorted {
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// Check one entry against its cell type's rules. Every entry is an object
/// with `source` in [`ENTRY_SOURCES`] and an `id` of 1..=128 chars with no
/// control chars.
/// - CONSTRAINT: `kind` in `forbid_edge | no_cycle | invariant`; forbid_edge
///   needs `from` and `to` (scope strings); `categories`, when present, are
///   edge category NAMES; invariant needs `text`. (The `[[constraint]]` rules
///   of `glia_config`, sharing its constants.)
/// - DECISION: `title` or `text`; `status` optional (stored lowercased).
/// - CONV: `text` of 1..=4096 chars; `by` and `at` optional strings (`at` is
///   the caller's opaque timestamp: glia never reads the clock).
pub fn validate_entry(cell: CellTypeId, v: &Value) -> Result<(), String> {
    let obj = v.as_object().ok_or("entry must be a JSON object")?;
    let text = |key: &str| string_field(obj, key);
    let present = |key: &str| -> Result<bool, String> {
        Ok(string_field(obj, key)?.is_some_and(|s| !s.trim().is_empty()))
    };
    match text("source")? {
        Some(s) if ENTRY_SOURCES.contains(&s) => {}
        other => return Err(format!("`source` {other:?} is not one of {}", ENTRY_SOURCES.join(" | "))),
    }
    let id = text("id")?.ok_or("`id` is required")?;
    let n = id.chars().count();
    if n == 0 || n > MAX_ID_CHARS || id.chars().any(char::is_control) {
        return Err(format!("`id` {id:?} must be 1..={MAX_ID_CHARS} chars with no control chars"));
    }
    match cell {
        c if c == cell_type::CONSTRAINT => {
            let kind = text("kind")?.unwrap_or_default();
            if !CONSTRAINT_KINDS.contains(&kind) {
                return Err(format!("`kind` {kind:?} is not one of {}", CONSTRAINT_KINDS.join(" | ")));
            }
            if kind == "forbid_edge" && !(present("from")? && present("to")?) {
                return Err("kind forbid_edge needs `from` and `to`".into());
            }
            if kind == "invariant" && !present("text")? {
                return Err("kind invariant needs `text`".into());
            }
            text("scope")?;
            match obj.get("categories") {
                None | Some(Value::Null) => {}
                Some(Value::Array(names)) => {
                    for name in names {
                        let known = name
                            .as_str()
                            .is_some_and(|n| edge_category::ALL.iter().any(|(_, e)| *e == n));
                        if !known {
                            return Err(format!("category {name} is not an edge category name"));
                        }
                    }
                }
                Some(_) => return Err("`categories` must be an array of edge category names".into()),
            }
            Ok(())
        }
        c if c == cell_type::DECISION => {
            text("status")?;
            if !(present("title")? || present("text")?) {
                return Err("a DECISION needs `title` or `text`".into());
            }
            Ok(())
        }
        c if c == cell_type::CONV => {
            let t = text("text")?.unwrap_or_default();
            let chars = t.chars().count();
            if t.trim().is_empty() || chars > MAX_NOTE_CHARS {
                return Err(format!("`text` must be 1..={MAX_NOTE_CHARS} chars (got {chars})"));
            }
            text("by")?;
            text("at")?;
            Ok(())
        }
        c if c == cell_type::VECTOR => Err("VECTOR takes a vector payload, not an entry".into()),
        c => Err(format!("cell type {} takes no external entries", c.0)),
    }
}

/// An optional string field: absent and `null` are `None`, any other type errs.
fn string_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

/// Upsert entry `v` into an entry-array payload: the entry replaces the one
/// with its `(source, id)`, the array is sorted by `(source, id)` and
/// serialized compactly with sorted keys. `existing` must be absent or a JSON
/// array - any other payload belongs to an extractor and is never clobbered.
/// A DECISION's `status` is stored lowercased.
pub fn merge_entry(existing: Option<&CellPayload>, v: &Value) -> Result<CellPayload, String> {
    let mut entries = match existing {
        None => Vec::new(),
        Some(p) => entry_array(p).ok_or("the existing cell is not an entry array")?,
    };
    let mut entry = canonical(v);
    if let Some(Value::String(s)) = entry.get_mut("status") {
        *s = s.to_lowercase();
    }
    let key = entry_key(&entry);
    entries.retain(|e| entry_key(e) != key);
    entries.push(entry);
    Ok(entries_payload(entries))
}

/// Drop the `(source, id)` entry. `None` when the array empties (the caller
/// drops the cell); a payload that is not an entry array is returned as is.
pub fn remove_entry(existing: &CellPayload, source: &str, id: &str) -> Option<CellPayload> {
    let Some(mut entries) = entry_array(existing) else {
        return Some(existing.clone());
    };
    entries.retain(|e| entry_key(e) != (source.to_string(), id.to_string()));
    (!entries.is_empty()).then(|| entries_payload(entries))
}

/// The entries of a JSON-array payload.
fn entry_array(p: &CellPayload) -> Option<Vec<Value>> {
    let CellPayload::Json(s) = p else {
        return None;
    };
    match serde_json::from_str::<Value>(s) {
        Ok(Value::Array(a)) => Some(a),
        _ => None,
    }
}

/// `(source, id)`; `""` for a missing field.
fn entry_key(e: &Value) -> (String, String) {
    let field = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    (field("source"), field("id"))
}

fn entries_payload(mut entries: Vec<Value>) -> CellPayload {
    entries.sort_by_key(entry_key);
    let arr = canonical(&Value::Array(entries));
    // Serializing a `Value` cannot fail (string keys, no custom impls).
    CellPayload::Json(serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into()))
}

/// What a declared rule forbids. `#[non_exhaustive]`: a reader outside this
/// crate keeps a `_` arm, which is where a kind it cannot evaluate goes
/// (listed as unchecked, never dropped).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConstraintKind {
    /// No edge from a node in scope `from` to a node in scope `to`.
    ForbidEdge { from: String, to: String },
    /// No cycle among the nodes in `scope` (`None` = the whole graph).
    NoCycle { scope: Option<String> },
    /// A statement no graph query checks.
    Invariant { text: String },
}

impl ConstraintKind {
    /// The stored `kind` name, one of `glia_config::CONSTRAINT_KINDS`.
    pub fn name(&self) -> &'static str {
        match self {
            ConstraintKind::ForbidEdge { .. } => "forbid_edge",
            ConstraintKind::NoCycle { .. } => "no_cycle",
            ConstraintKind::Invariant { .. } => "invariant",
        }
    }
}

/// One CONSTRAINT entry, typed: the rule a checker evaluates.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConstraintRule {
    pub id: String,
    pub kind: ConstraintKind,
    /// Edge category NAMES the rule is restricted to, as stored (empty = the
    /// checker's default set). A name no longer registered is kept, so the
    /// checker can report it instead of the rule vanishing.
    pub categories: Vec<String>,
    /// Who wrote it: one of [`ENTRY_SOURCES`].
    pub source: String,
    /// `.glia/overlay.toml:<line>` for a declared rule; `None` for an API write.
    pub decl: Option<String>,
}

/// Every well-formed rule in a CONSTRAINT payload, in stored order (sorted by
/// `(source, id)`). An entry that is not an object, lacks a string `source` /
/// `id`, names an unknown `kind`, lacks its kind's required field
/// (forbid_edge `from` + `to`, invariant `text`) or has a non-string where a
/// string belongs is skipped; a payload that is not a JSON array yields
/// nothing. Never panics.
pub fn parse_constraints(payload: &CellPayload) -> Vec<ConstraintRule> {
    entry_array(payload).unwrap_or_default().iter().filter_map(constraint_rule).collect()
}

fn constraint_rule(v: &Value) -> Option<ConstraintRule> {
    let obj = v.as_object()?;
    // `Err` (a non-string) is malformed; `Ok(None)` is absent.
    let field = |key: &str| string_field(obj, key).ok();
    let required = |key: &str| {
        field(key).flatten().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
    };
    let kind = match field("kind")?? {
        "forbid_edge" => ConstraintKind::ForbidEdge { from: required("from")?, to: required("to")? },
        "no_cycle" => ConstraintKind::NoCycle { scope: field("scope")?.map(String::from) },
        "invariant" => ConstraintKind::Invariant { text: required("text")? },
        _ => return None,
    };
    let categories = match obj.get("categories") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(names)) => {
            names.iter().map(|n| n.as_str().map(String::from)).collect::<Option<Vec<_>>>()?
        }
        Some(_) => return None,
    };
    Some(ConstraintRule {
        id: required("id")?,
        kind,
        categories,
        source: required("source")?,
        decl: field("decl")?.map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("glia_ext_inputs_{}_{tag}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn merge_upserts_sorts_and_serializes_compactly() {
        let a = json!({"text": "b", "source": "api", "id": "2"});
        let b = json!({"source": "api", "id": "1", "text": "a"});
        let p = merge_entry(None, &a).unwrap();
        let p = merge_entry(Some(&p), &b).unwrap();
        assert_eq!(
            p,
            CellPayload::Json(r#"[{"id":"1","source":"api","text":"a"},{"id":"2","source":"api","text":"b"}]"#.into())
        );
        // Same (source, id) replaces.
        let p = merge_entry(Some(&p), &json!({"source": "api", "id": "1", "text": "z"})).unwrap();
        assert_eq!(
            p,
            CellPayload::Json(r#"[{"id":"1","source":"api","text":"z"},{"id":"2","source":"api","text":"b"}]"#.into())
        );
        // An extractor-owned payload is never clobbered.
        assert!(merge_entry(Some(&CellPayload::Text("x".into())), &a).is_err());
        assert!(merge_entry(Some(&CellPayload::Json("{}".into())), &a).is_err());
        // DECISION status is stored lowercased.
        let d = merge_entry(None, &json!({"source": "api", "id": "d", "title": "t", "status": "Accepted"})).unwrap();
        assert_eq!(d, CellPayload::Json(r#"[{"id":"d","source":"api","status":"accepted","title":"t"}]"#.into()));
    }

    #[test]
    fn remove_drops_the_entry_and_empties_to_none() {
        let p = merge_entry(None, &json!({"source": "api", "id": "1", "text": "a"})).unwrap();
        let p = merge_entry(Some(&p), &json!({"source": "adr", "id": "1", "title": "t"})).unwrap();
        let p = remove_entry(&p, "api", "1").unwrap();
        assert_eq!(p, CellPayload::Json(r#"[{"id":"1","source":"adr","title":"t"}]"#.into()));
        assert_eq!(remove_entry(&p, "adr", "1"), None);
        let text = CellPayload::Text("x".into());
        assert_eq!(remove_entry(&text, "api", "1"), Some(text));
    }

    #[test]
    fn validate_entry_rules() {
        let ok = |c, v: Value| validate_entry(c, &v).is_ok();
        assert!(ok(cell_type::CONV, json!({"source": "api", "id": "1", "text": "t", "by": "me", "at": "x"})));
        assert!(!ok(cell_type::CONV, json!({"source": "api", "id": "1", "text": "  "})));
        assert!(!ok(cell_type::CONV, json!({"source": "api", "id": "1", "text": "x".repeat(MAX_NOTE_CHARS + 1)})));
        assert!(!ok(cell_type::CONV, json!({"source": "web", "id": "1", "text": "t"})));
        assert!(!ok(cell_type::CONV, json!({"source": "api", "id": "", "text": "t"})));
        assert!(!ok(cell_type::CONV, json!({"source": "api", "id": "a\u{7}", "text": "t"})));
        assert!(!ok(cell_type::CONV, json!({"source": "api", "id": "x".repeat(MAX_ID_CHARS + 1), "text": "t"})));
        assert!(!ok(cell_type::CONV, json!(["not", "an", "object"])));
        assert!(ok(cell_type::DECISION, json!({"source": "adr", "id": "d", "text": "t"})));
        assert!(!ok(cell_type::DECISION, json!({"source": "adr", "id": "d", "status": "open"})));
        assert!(ok(
            cell_type::CONSTRAINT,
            json!({"source": "overlay", "id": "c", "kind": "forbid_edge", "from": "a", "to": "b", "categories": ["CALLS"]})
        ));
        assert!(!ok(cell_type::CONSTRAINT, json!({"source": "overlay", "id": "c", "kind": "forbid_edge", "from": "a"})));
        assert!(!ok(
            cell_type::CONSTRAINT,
            json!({"source": "overlay", "id": "c", "kind": "no_cycle", "categories": ["NOPE"]})
        ));
        assert!(ok(cell_type::CONSTRAINT, json!({"source": "overlay", "id": "c", "kind": "no_cycle"})));
        assert!(!ok(cell_type::CONSTRAINT, json!({"source": "overlay", "id": "c", "kind": "invariant"})));
        assert!(!ok(cell_type::CONSTRAINT, json!({"source": "overlay", "id": "c", "kind": "other"})));
        assert!(!ok(cell_type::VECTOR, json!({"source": "api", "id": "v"})));
        assert!(!ok(cell_type::CODE, json!({"source": "api", "id": "v"})));
    }

    #[test]
    fn parse_constraints_reads_every_kind_and_skips_malformed() {
        let entries = [
            json!({"source": "overlay", "id": "a", "kind": "forbid_edge", "from": "web", "to": "services/api",
                   "from_raw": "@shop/web", "categories": ["CALLS", "RENAMED_SINCE"], "decl": ".glia/overlay.toml:3"}),
            json!({"source": "overlay", "id": "b", "kind": "no_cycle", "scope": "services/api"}),
            json!({"source": "api", "id": "c", "kind": "no_cycle"}),
            json!({"source": "api", "id": "d", "kind": "invariant", "text": "charges are idempotent"}),
            // Malformed: each is skipped, never a panic.
            json!({"source": "api", "id": "e", "kind": "forbid_edge", "from": "web"}),
            json!({"source": "api", "id": "f", "kind": "invariant", "text": "  "}),
            json!({"source": "api", "id": "g", "kind": "other"}),
            json!({"source": "api", "kind": "no_cycle"}),
            json!({"id": "h", "kind": "no_cycle"}),
            json!({"source": "api", "id": "i", "kind": "no_cycle", "scope": 7}),
            json!({"source": "api", "id": "j", "kind": "no_cycle", "categories": ["CALLS", 1]}),
            json!({"source": "api", "id": "k", "kind": "no_cycle", "categories": "CALLS"}),
            json!(["not", "an", "object"]),
        ];
        let payload = CellPayload::Json(serde_json::to_string(&entries).unwrap());
        let rules = parse_constraints(&payload);
        let got: Vec<(&str, &str, Option<&str>)> =
            rules.iter().map(|r| (r.id.as_str(), r.kind.name(), r.decl.as_deref())).collect();
        assert_eq!(
            got,
            [
                ("a", "forbid_edge", Some(".glia/overlay.toml:3")),
                ("b", "no_cycle", None),
                ("c", "no_cycle", None),
                ("d", "invariant", None),
            ]
        );
        assert_eq!(rules[0].kind, ConstraintKind::ForbidEdge { from: "web".into(), to: "services/api".into() });
        assert_eq!(rules[0].categories, ["CALLS", "RENAMED_SINCE"], "an unregistered name is kept for the checker");
        assert_eq!(rules[0].source, "overlay");
        assert_eq!(rules[1].kind, ConstraintKind::NoCycle { scope: Some("services/api".into()) });
        assert_eq!(rules[2].kind, ConstraintKind::NoCycle { scope: None });
        assert_eq!(rules[3].kind, ConstraintKind::Invariant { text: "charges are idempotent".into() });

        // Round trip through the writer's merge: what merge_entry stores parses back.
        let stored = merge_entry(None, &entries[0]).unwrap();
        assert_eq!(parse_constraints(&stored), rules[..1]);
        // A payload that is not an entry array yields nothing.
        assert!(parse_constraints(&CellPayload::Json("{}".into())).is_empty());
        assert!(parse_constraints(&CellPayload::Text("not json".into())).is_empty());
        assert!(parse_constraints(&CellPayload::Bytes(vec![1, 2])).is_empty());
    }

    #[test]
    fn rows_roundtrip_and_bad_lines_are_reported() {
        let dir = tmp("rows");
        let path = dir.join(".glia").join("cells.jsonl");
        let rows = vec![CellRow {
            qname: "a::f".into(),
            kind: None,
            hint: None,
            cell: "CONV".into(),
            entry: json!({"text": "t", "source": "api", "id": "1"}),
        }];
        write_rows(&path, &rows).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            written,
            "{\"cell\":\"CONV\",\"entry\":{\"id\":\"1\",\"source\":\"api\",\"text\":\"t\"},\"qname\":\"a::f\"}\n"
        );
        assert!(!dir.join(".glia").join("cells.jsonl.tmp").exists());
        std::fs::write(&path, format!("{written}\nnot json\n{{\"qname\":\"x\",\"cell\":\"CONV\",\"entry\":1,\"extra\":1}}\n")).unwrap();
        let (back, errors): (Vec<CellRow>, _) = read_rows(&path);
        assert_eq!(back, rows);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("cells.jsonl:3: "), "{errors:?}");
        assert!(errors[1].contains("cells.jsonl:4: "), "{errors:?}");
        let (none, errs): (Vec<CellRow>, _) = read_rows(&dir.join("missing.jsonl"));
        assert!(none.is_empty() && errs.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn vector_rows_decode_and_check_dims() {
        let bytes = vec![0u8, 0, 128, 63, 0, 0, 0, 64];
        assert_eq!(b64_encode(&bytes), "AACAPwAAAEA=");
        let row = |dims| VectorRow {
            qname: "a::f".into(),
            kind: None,
            hint: None,
            model: None,
            dims,
            b64: "AACAPwAAAEA=".into(),
        };
        let w = CellWrite::try_from(&row(Some(2))).unwrap();
        assert_eq!(w.payload, WritePayload::Vector { bytes: bytes.clone(), model: None, dims: Some(2) });
        assert!(CellWrite::try_from(&row(Some(3))).is_err());
        assert!(CellWrite::try_from(&row(None)).is_ok());
        let bad = VectorRow { b64: "***".into(), ..row(None) };
        assert!(CellWrite::try_from(&bad).is_err());
        assert!(check_vector(&vec![0; MAX_VECTOR_BYTES + 4], None).is_err());
        assert!(check_vector(&[], None).is_err());
        let unknown = CellRow { qname: "a".into(), kind: None, hint: None, cell: "NOPE".into(), entry: json!({}) };
        assert!(CellWrite::try_from(&unknown).is_err());
    }
}
