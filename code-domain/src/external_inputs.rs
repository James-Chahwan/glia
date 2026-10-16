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
//! crate (`glia_graph::cells`), so the build, a live in-memory write and
//! a persisted write-through bind a row to the same node.
//!
//! CONSTRAINT ENTRIES (the rule schema `glia check`, LE.8, reads through
//! [`parse_constraints`] and nowhere else). The overlay's `[[constraint]]`
//! stanzas (LF.4a) are stored as
//! `{"source":"overlay","id","kind","from"?,"to"?,"from_raw"?,"to_raw"?,"scope"?,"scope_raw"?,"categories"?,"text"?,"transitive"?,"via"?,"via_raw"?,"rule"?,"min_tier"?,"origin":"human|llm","decl":".glia/overlay.toml:<line>"}`:
//! `from` / `to` / `scope` / `via` hold the scope RESOLVED to a repo-relative
//! path (`.` = the repo root) and the `*_raw` keys what the stanza said (a
//! project label, a qname or a path), so a reader never re-resolves a label.
//! An entry written through the cell API (`source` `api`) carries the scope
//! strings as its writer gave them.
//!
//! The version-2 keys (DB.1, overlay `version = 2`) are written only when set,
//! so a version-1 stanza's entry is what 0.5.1 stored: `transitive` (`true`)
//! and `via` on a forbid_edge, `rule` (Datalog text) on an invariant,
//! `min_tier` (`fact | derived`, `glia_config::MIN_TIERS`). A `rule` is
//! declared only in `.glia/overlay.toml` (0.5.2 Q9): [`validate_entry`], the
//! cell API's check, refuses the key whatever `source` says, and only the
//! overlay stage checks its entries through [`validate_declared_entry`].
//!
//! THE REFLEXION MODEL (CC.5a) is stored the same way, so a loaded `.gmap` can
//! be checked without the overlay file:
//! - `[[component]]` -> `{"source":"overlay","id":"component:<name>","kind":"component","name","paths":[resolved],"paths_raw":[..],"text"?,"origin","decl"}`;
//! - `[[layer]]` -> `{"source":"overlay","id":"layer:<name>","kind":"layer","name","rank":<0-based, top first>,"components":[..],"strict":bool,"decl"}`;
//! - `[[constraint]] kind = "allow"` -> `{"source":"overlay","id","kind":"allow","from","to","categories"?,"text"?,"origin","decl"}`,
//!   `from` / `to` the component NAMES as written (they are not scopes).

use std::collections::BTreeMap;
use std::io::{self, Write as _};
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use glia_core::{CellPayload, CellTypeId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::cell_type;
use crate::edge_category;
use crate::glia_config::{
    CONSTRAINT_KINDS, MAX_ID_CHARS, MAX_NOTE_CHARS, MIN_TIERS, MODEL_KINDS, RULE_ONLY_INVARIANT,
    TRANSITIVE_ONLY_FORBID, VIA_ONLY_TRANSITIVE, check_min_tier, check_min_tier_on, check_rule_text,
};

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

/// The Q9 refusal: the cell API cannot write a `rule` (0.5.2 Q9).
pub const RULE_IS_OVERLAY_ONLY: &str = "`rule` (Datalog text) is declared only in .glia/overlay.toml [[constraint]] (0.5.2 Q9); the cell API cannot write one";

/// Check one entry against its cell type's rules: the check of every cell-API
/// path (`glia_store::cells::write_cell` for `glia cell set` and pyo3's
/// `write_cell`, `glia_graph::cells::apply_cell_write` for the build's
/// `.glia/cells.jsonl` stage and pyo3's `set_cell`). Every entry is an object
/// with `source` in [`ENTRY_SOURCES`] and an `id` of 1..=128 chars with no
/// control chars.
/// - CONSTRAINT: `kind` in `forbid_edge | no_cycle | invariant | allow`
///   (`glia_config::CONSTRAINT_KINDS`) or `component | layer`
///   (`glia_config::MODEL_KINDS`); forbid_edge needs `from` and `to` (scope
///   strings), allow `from` and `to` (component names); `categories`, when
///   present, are edge category NAMES; invariant needs `text`; component needs
///   `name` and a non-empty `paths` array of non-empty strings; layer needs
///   `name`, a u32 `rank` and a non-empty `components` array of non-empty
///   strings, and `strict`, when present, is a bool. The version-2 keys
///   (DB.1): `transitive`, when present, is a bool, and `true` only on a
///   forbid_edge; `via` is a non-empty array of non-empty strings, only with
///   `transitive` true; `min_tier` is one of `glia_config::MIN_TIERS`, on a
///   forbid_edge or no_cycle (on an invariant only with a `rule`, which this
///   check never admits). A `rule` key of ANY value is refused whatever
///   `source` says ([`RULE_IS_OVERLAY_ONLY`], 0.5.2 Q9). (The `[[constraint]]`
///   / `[[component]]` / `[[layer]]` rules of `glia_config`, sharing its
///   constants.)
/// - DECISION: `title` or `text`; `status` optional (stored lowercased).
/// - CONV: `text` of 1..=4096 chars; `by` and `at` optional strings (`at` is
///   the caller's opaque timestamp: glia never reads the clock).
pub fn validate_entry(cell: CellTypeId, v: &Value) -> Result<(), String> {
    check_entry(cell, v, false)
}

/// [`validate_entry`]'s checks, plus a CONSTRAINT `rule`: a non-blank string
/// of at most `glia_config::MAX_RULE_CHARS` chars, on kind invariant, from
/// source `overlay`. Called ONLY by the overlay stage, which builds its
/// entries from `.glia/overlay.toml` (`engine::external::declared`): a second
/// caller would reopen the Q9 bypass. The rule's text is not parsed here
/// (DB.3a parses it at `glia check` time).
pub fn validate_declared_entry(cell: CellTypeId, v: &Value) -> Result<(), String> {
    check_entry(cell, v, true)
}

/// The shared body of [`validate_entry`] (`allow_rule` false) and
/// [`validate_declared_entry`] (`allow_rule` true).
fn check_entry(cell: CellTypeId, v: &Value, allow_rule: bool) -> Result<(), String> {
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
        c if c == cell_type::CONSTRAINT => constraint_checks(obj, allow_rule),
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

/// A CONSTRAINT entry's rules (see [`validate_entry`]); `allow_rule` admits a
/// `rule` on an overlay invariant ([`validate_declared_entry`]).
fn constraint_checks(obj: &Map<String, Value>, allow_rule: bool) -> Result<(), String> {
    let text = |key: &str| string_field(obj, key);
    let present = |key: &str| -> Result<bool, String> {
        Ok(string_field(obj, key)?.is_some_and(|s| !s.trim().is_empty()))
    };
    let kind = text("kind")?.unwrap_or_default();
    if !CONSTRAINT_KINDS.contains(&kind) && !MODEL_KINDS.contains(&kind) {
        let all: Vec<&str> = CONSTRAINT_KINDS.iter().chain(MODEL_KINDS).copied().collect();
        return Err(format!("`kind` {kind:?} is not one of {}", all.join(" | ")));
    }
    if matches!(kind, "forbid_edge" | "allow") && !(present("from")? && present("to")?) {
        return Err(format!("kind {kind} needs `from` and `to`"));
    }
    if kind == "invariant" && !present("text")? {
        return Err("kind invariant needs `text`".into());
    }
    if matches!(kind, "component" | "layer") && !present("name")? {
        return Err(format!("kind {kind} needs `name`"));
    }
    if kind == "component" && string_list(obj, "paths").is_none() {
        return Err("kind component needs `paths`: a non-empty array of non-empty strings".into());
    }
    if kind == "layer" {
        if obj.get("rank").and_then(Value::as_u64).and_then(|r| u32::try_from(r).ok()).is_none() {
            return Err("kind layer needs `rank`: an integer 0..=4294967295".into());
        }
        if string_list(obj, "components").is_none() {
            return Err("kind layer needs `components`: a non-empty array of non-empty strings".into());
        }
        if !matches!(obj.get("strict"), None | Some(Value::Null | Value::Bool(_))) {
            return Err("`strict` must be a bool".into());
        }
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
    // The version-2 keys (DB.1).
    let has_rule = match obj.get("rule") {
        None => false,
        Some(_) if !allow_rule => return Err(RULE_IS_OVERLAY_ONLY.into()),
        Some(Value::String(rule)) => {
            if kind != "invariant" {
                return Err(RULE_ONLY_INVARIANT.into());
            }
            check_rule_text(rule)?;
            match text("source")? {
                Some("overlay") => {}
                other => {
                    let src = other.unwrap_or_default();
                    return Err(format!("`rule` is written only with source `overlay`, not {src:?} (0.5.2 Q9)"));
                }
            }
            true
        }
        Some(_) => return Err("`rule` must be a string of Datalog text".into()),
    };
    let transitive = match obj.get("transitive") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("`transitive` must be a bool".into()),
    };
    if transitive && kind != "forbid_edge" {
        return Err(TRANSITIVE_ONLY_FORBID.into());
    }
    match obj.get("via") {
        None | Some(Value::Null) => {}
        Some(_) if string_list(obj, "via").is_none() => {
            return Err("`via` must be a non-empty array of non-empty strings".into());
        }
        Some(_) if !transitive => return Err(VIA_ONLY_TRANSITIVE.into()),
        Some(_) => {}
    }
    match obj.get("min_tier") {
        None | Some(Value::Null) => {}
        Some(Value::String(tier)) => {
            check_min_tier(tier)?;
            check_min_tier_on(kind, has_rule)?;
        }
        Some(_) => return Err(format!("`min_tier` must be a string: {}", MIN_TIERS.join(" | "))),
    }
    Ok(())
}

/// `obj[key]` as a non-empty array of non-empty (trimmed) strings; `None` for
/// anything else.
fn string_list(obj: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    let items = obj.get(key)?.as_array()?;
    let out = items
        .iter()
        .map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from))
        .collect::<Option<Vec<_>>>()?;
    (!out.is_empty()).then_some(out)
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
    /// No edge from a node in scope `from` to a node in scope `to`. With
    /// `transitive` (overlay version 2, DB.1), no path either: a path through
    /// any third scope counts, except one that enters `to` through a `via`
    /// facade scope (resolved paths, like `from` / `to`). A transitive rule is
    /// evaluated as `forbid_path` violations by `engine::check` (DB.3b).
    ForbidEdge { from: String, to: String, transitive: bool, via: Vec<String> },
    /// No cycle among the nodes in `scope` (`None` = the whole graph).
    NoCycle { scope: Option<String> },
    /// A statement. With `rule` (overlay version 2, DB.1) it is checked: the
    /// Datalog text's `violation/N` rows are its violations (DB.3a); without
    /// one no graph query checks it.
    Invariant { text: String, rule: Option<String> },
    /// A reflexion-model component (CC.5a): the code under `paths` (resolved
    /// repo-relative paths, `.` = the root) belongs to `name`. A declaration,
    /// not a rule.
    Component { name: String, paths: Vec<String> },
    /// A reflexion-model layer (CC.5a): `rank` 0 is the top. Its components
    /// may use those of any lower layer, or with `strict` only of rank + 1. A
    /// declaration, not a rule.
    Layer { name: String, rank: u32, components: Vec<String>, strict: bool },
    /// The model permits component `from` to depend on component `to` (CC.5a).
    Allow { from: String, to: String },
}

impl ConstraintKind {
    /// The stored `kind` name, one of `glia_config::CONSTRAINT_KINDS` or
    /// `glia_config::MODEL_KINDS`.
    pub fn name(&self) -> &'static str {
        match self {
            ConstraintKind::ForbidEdge { .. } => "forbid_edge",
            ConstraintKind::NoCycle { .. } => "no_cycle",
            ConstraintKind::Invariant { .. } => "invariant",
            ConstraintKind::Component { .. } => "component",
            ConstraintKind::Layer { .. } => "layer",
            ConstraintKind::Allow { .. } => "allow",
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
    /// The weakest violation tier that fails CI: one of
    /// `glia_config::MIN_TIERS` (`fact | derived`); `None` = the checker's
    /// default (DB.1, read by DB.3a).
    pub min_tier: Option<String>,
}

/// Every well-formed rule in a CONSTRAINT payload, in stored order (sorted by
/// `(source, id)`). An entry that is not an object, lacks a string `source` /
/// `id`, names an unknown `kind`, lacks its kind's required field
/// (forbid_edge / allow `from` + `to`, invariant `text`, component `name` +
/// a non-empty `paths` array of non-empty strings, layer `name` + a u32
/// `rank` + a non-empty `components` array of non-empty strings) or has a
/// non-string where a string belongs (a non-bool `strict` or forbid_edge
/// `transitive`, a forbid_edge `via` that is not a non-empty array of
/// non-empty strings, a blank or non-string invariant `rule`, a `min_tier`
/// outside `glia_config::MIN_TIERS`) is skipped; a payload that is not a JSON
/// array yields nothing. Never panics.
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
        "forbid_edge" => ConstraintKind::ForbidEdge {
            from: required("from")?,
            to: required("to")?,
            transitive: match obj.get("transitive") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return None,
            },
            via: match obj.get("via") {
                None | Some(Value::Null) => Vec::new(),
                Some(_) => string_list(obj, "via")?,
            },
        },
        "no_cycle" => ConstraintKind::NoCycle { scope: field("scope")?.map(String::from) },
        "invariant" => ConstraintKind::Invariant {
            text: required("text")?,
            rule: match field("rule")? {
                None => None,
                Some(r) if r.trim().is_empty() => return None,
                Some(r) => Some(r.to_string()),
            },
        },
        "allow" => ConstraintKind::Allow { from: required("from")?, to: required("to")? },
        "component" => ConstraintKind::Component { name: required("name")?, paths: string_list(obj, "paths")? },
        "layer" => ConstraintKind::Layer {
            name: required("name")?,
            rank: u32::try_from(obj.get("rank")?.as_u64()?).ok()?,
            components: string_list(obj, "components")?,
            strict: match obj.get("strict") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return None,
            },
        },
        _ => return None,
    };
    let categories = match obj.get("categories") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(names)) => {
            names.iter().map(|n| n.as_str().map(String::from)).collect::<Option<Vec<_>>>()?
        }
        Some(_) => return None,
    };
    let min_tier = match field("min_tier")? {
        None => None,
        Some(t) if MIN_TIERS.contains(&t) => Some(t.to_string()),
        Some(_) => return None,
    };
    Some(ConstraintRule {
        id: required("id")?,
        kind,
        categories,
        source: required("source")?,
        decl: field("decl")?.map(String::from),
        min_tier,
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
        // CC.5a: allow needs from + to; component / layer validate like an overlay write.
        assert!(ok(cell_type::CONSTRAINT, json!({"source": "api", "id": "a", "kind": "allow", "from": "web", "to": "api"})));
        assert!(!ok(cell_type::CONSTRAINT, json!({"source": "api", "id": "a", "kind": "allow", "from": "web"})));
        assert!(!ok(cell_type::CONSTRAINT, json!({"source": "api", "id": "a", "kind": "allow", "to": "api"})));
        assert!(ok(
            cell_type::CONSTRAINT,
            json!({"source": "api", "id": "component:web", "kind": "component", "name": "web", "paths": ["web"]})
        ));
        for bad in [json!([]), json!([""]), json!("web"), json!([1])] {
            assert!(!ok(
                cell_type::CONSTRAINT,
                json!({"source": "api", "id": "component:web", "kind": "component", "name": "web", "paths": bad})
            ));
        }
        assert!(!ok(cell_type::CONSTRAINT, json!({"source": "api", "id": "component:x", "kind": "component", "paths": ["x"]})));
        assert!(ok(
            cell_type::CONSTRAINT,
            json!({"source": "api", "id": "layer:ui", "kind": "layer", "name": "ui", "rank": 0, "components": ["web"], "strict": true})
        ));
        for (rank, components, strict) in [
            (json!(-1), json!(["web"]), json!(false)),
            (json!(1.5), json!(["web"]), json!(false)),
            (json!(0), json!([]), json!(false)),
            (json!(0), json!(["web"]), json!("yes")),
        ] {
            assert!(!ok(
                cell_type::CONSTRAINT,
                json!({"source": "api", "id": "layer:ui", "kind": "layer", "name": "ui", "rank": rank, "components": components, "strict": strict})
            ));
        }
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
            json!({"source": "overlay", "id": "component:web", "kind": "component", "name": "web",
                   "paths": ["web", "shared/ui"], "paths_raw": ["web", "./shared/ui"], "decl": ".glia/overlay.toml:20"}),
            json!({"source": "overlay", "id": "layer:core", "kind": "layer", "name": "core", "rank": 1,
                   "components": ["api"], "strict": true}),
            json!({"source": "overlay", "id": "web-uses-api", "kind": "allow", "from": "web", "to": "api", "categories": ["CALLS"]}),
            // Malformed: each is skipped, never a panic.
            json!({"source": "api", "id": "l", "kind": "component", "name": "x", "paths": []}),
            json!({"source": "api", "id": "m", "kind": "layer", "name": "x", "rank": 1.5, "components": ["x"]}),
            json!({"source": "api", "id": "n", "kind": "layer", "name": "x", "rank": 0, "components": ["x"], "strict": "no"}),
            json!({"source": "api", "id": "o", "kind": "allow", "from": "web"}),
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
                ("component:web", "component", Some(".glia/overlay.toml:20")),
                ("layer:core", "layer", None),
                ("web-uses-api", "allow", None),
            ]
        );
        assert_eq!(
            rules[0].kind,
            ConstraintKind::ForbidEdge { from: "web".into(), to: "services/api".into(), transitive: false, via: vec![] }
        );
        assert_eq!(rules[0].categories, ["CALLS", "RENAMED_SINCE"], "an unregistered name is kept for the checker");
        assert_eq!(rules[0].source, "overlay");
        assert_eq!(rules[1].kind, ConstraintKind::NoCycle { scope: Some("services/api".into()) });
        assert_eq!(rules[2].kind, ConstraintKind::NoCycle { scope: None });
        assert_eq!(rules[3].kind, ConstraintKind::Invariant { text: "charges are idempotent".into(), rule: None });
        assert_eq!(
            rules[4].kind,
            ConstraintKind::Component { name: "web".into(), paths: vec!["web".into(), "shared/ui".into()] }
        );
        assert_eq!(
            rules[5].kind,
            ConstraintKind::Layer { name: "core".into(), rank: 1, components: vec!["api".into()], strict: true }
        );
        assert_eq!(rules[6].kind, ConstraintKind::Allow { from: "web".into(), to: "api".into() });
        assert_eq!(rules[6].categories, ["CALLS"]);
        // strict defaults to false.
        let lax = json!([{"source": "api", "id": "l", "kind": "layer", "name": "l", "rank": 0, "components": ["web"]}]);
        let lax = parse_constraints(&CellPayload::Json(lax.to_string()));
        assert!(matches!(lax[0].kind, ConstraintKind::Layer { strict: false, .. }), "{lax:?}");

        // Round trip through the writer's merge: what merge_entry stores parses back.
        let stored = merge_entry(None, &entries[0]).unwrap();
        assert_eq!(parse_constraints(&stored), rules[..1]);
        // A payload that is not an entry array yields nothing.
        assert!(parse_constraints(&CellPayload::Json("{}".into())).is_empty());
        assert!(parse_constraints(&CellPayload::Text("not json".into())).is_empty());
        assert!(parse_constraints(&CellPayload::Bytes(vec![1, 2])).is_empty());
    }

    /// DB.1 / Q9: the cell API (validate_entry) refuses a `rule` whatever its
    /// `source` says - the HEAD probes stored both entries below - and only
    /// the overlay stage's check (validate_declared_entry) admits one.
    #[test]
    fn validate_entry_rule_fields() {
        let rule = "violation(f) :- node(f, \"FUNCTION\").";
        let probe = |source: &str| json!({"source": source, "id": "r1", "kind": "invariant", "text": "t", "rule": rule});
        for source in ["api", "overlay"] {
            let e = validate_entry(cell_type::CONSTRAINT, &probe(source)).expect_err(source);
            assert!(e.contains("declared only in .glia/overlay.toml"), "{source}: {e}");
            assert_eq!(e, RULE_IS_OVERLAY_ONLY);
        }
        // Any value, even null, is refused: the key itself is overlay-only.
        for v in [json!(null), json!(""), json!(7)] {
            let entry = json!({"source": "api", "id": "r", "kind": "invariant", "text": "t", "rule": v});
            assert_eq!(validate_entry(cell_type::CONSTRAINT, &entry), Err(RULE_IS_OVERLAY_ONLY.to_string()));
        }
        assert_eq!(validate_declared_entry(cell_type::CONSTRAINT, &probe("overlay")), Ok(()));
        let api = validate_declared_entry(cell_type::CONSTRAINT, &probe("api")).expect_err("source api");
        assert!(api.contains("only with source `overlay`") && api.contains("Q9"), "{api}");
        let declared = |v: Value| validate_declared_entry(cell_type::CONSTRAINT, &v);
        let blank = declared(json!({"source": "overlay", "id": "r", "kind": "invariant", "text": "t", "rule": "  "}));
        assert!(blank.as_ref().is_err_and(|e| e.contains("not blank")), "{blank:?}");
        let long = "x".repeat(crate::glia_config::MAX_RULE_CHARS + 1);
        assert!(declared(json!({"source": "overlay", "id": "r", "kind": "invariant", "text": "t", "rule": long})).is_err());
        let on_forbid = declared(json!({"source": "overlay", "id": "r", "kind": "forbid_edge", "from": "a", "to": "b", "rule": rule}));
        assert_eq!(on_forbid, Err("`rule` is an invariant field".to_string()));
        assert!(declared(json!({"source": "overlay", "id": "r", "kind": "invariant", "text": "t", "rule": 1})).is_err());
        assert_eq!(
            declared(json!({"source": "overlay", "id": "r", "kind": "invariant", "text": "t", "rule": rule, "min_tier": "fact"})),
            Ok(())
        );
        // validate_declared_entry runs every validate_entry check besides.
        assert!(validate_declared_entry(cell_type::CONV, &json!({"source": "overlay", "id": "n", "text": "  "})).is_err());
        assert!(validate_declared_entry(cell_type::DECISION, &json!({"source": "overlay", "id": "d", "text": "t"})).is_ok());

        // transitive / via / min_tier: the same shape rules on both paths.
        let forbid = |extra: Value| {
            let mut e = json!({"source": "api", "id": "f", "kind": "forbid_edge", "from": "web", "to": "api"});
            for (k, v) in extra.as_object().unwrap() {
                e[k] = v.clone();
            }
            e
        };
        for check in [validate_entry, validate_declared_entry] {
            let ok = |v: Value| check(cell_type::CONSTRAINT, &v).is_ok();
            assert!(ok(forbid(json!({"transitive": true, "via": ["api/public"], "min_tier": "derived"}))));
            assert!(ok(forbid(json!({"transitive": false}))));
            assert!(ok(json!({"source": "api", "id": "c", "kind": "no_cycle", "min_tier": "fact"})));
            for (bad, needle) in [
                (forbid(json!({"min_tier": "heuristic"})), "never fails CI"),
                (forbid(json!({"min_tier": "x"})), "must be fact | derived"),
                (forbid(json!({"min_tier": 1})), "must be a string"),
                (forbid(json!({"transitive": "yes"})), "`transitive` must be a bool"),
                (forbid(json!({"via": ["api/public"]})), "set `transitive = true`"),
                (forbid(json!({"transitive": true, "via": []})), "non-empty array"),
                (forbid(json!({"transitive": true, "via": [""]})), "non-empty array"),
                (forbid(json!({"transitive": true, "via": "api/public"})), "non-empty array"),
                (json!({"source": "api", "id": "c", "kind": "no_cycle", "transitive": true}), "forbid_edge field"),
                (json!({"source": "api", "id": "c", "kind": "no_cycle", "via": ["x"]}), "set `transitive = true`"),
                (json!({"source": "api", "id": "a", "kind": "allow", "from": "w", "to": "a", "min_tier": "fact"}), "kind allow"),
                (json!({"source": "api", "id": "i", "kind": "invariant", "text": "t", "min_tier": "fact"}), "needs `rule`"),
                (
                    json!({"source": "api", "id": "component:w", "kind": "component", "name": "w", "paths": ["w"], "min_tier": "fact"}),
                    "kind component",
                ),
            ] {
                let e = check(cell_type::CONSTRAINT, &bad).expect_err(needle);
                assert!(e.contains(needle), "{bad}: {e}");
            }
        }
    }

    /// DB.1: the version-2 keys read into the new variant fields; a malformed
    /// one skips its entry (the reader's contract).
    #[test]
    fn parse_constraints_reads_version_two_fields() {
        let rule = "violation(x) :- node(x, \"ROUTE\").";
        let entries = [
            json!({"source": "overlay", "id": "a", "kind": "forbid_edge", "from": "web", "to": "services/api",
                   "transitive": true, "via": ["services/api/public"], "via_raw": ["./services/api/public/"],
                   "min_tier": "fact", "decl": ".glia/overlay.toml:3"}),
            json!({"source": "overlay", "id": "b", "kind": "invariant", "text": "no routes", "rule": rule,
                   "min_tier": "derived", "decl": ".glia/overlay.toml:12"}),
            json!({"source": "overlay", "id": "c", "kind": "no_cycle", "min_tier": "fact"}),
            // Malformed: each is skipped.
            json!({"source": "overlay", "id": "d", "kind": "forbid_edge", "from": "a", "to": "b", "transitive": "yes"}),
            json!({"source": "overlay", "id": "e", "kind": "forbid_edge", "from": "a", "to": "b", "transitive": true, "via": []}),
            json!({"source": "overlay", "id": "f", "kind": "forbid_edge", "from": "a", "to": "b", "transitive": true, "via": [3]}),
            json!({"source": "overlay", "id": "g", "kind": "invariant", "text": "t", "rule": "  "}),
            json!({"source": "overlay", "id": "h", "kind": "invariant", "text": "t", "rule": 1}),
            json!({"source": "overlay", "id": "i", "kind": "no_cycle", "min_tier": "heuristic"}),
        ];
        let payload = CellPayload::Json(serde_json::to_string(&entries).unwrap());
        let rules = parse_constraints(&payload);
        let ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(
            rules[0].kind,
            ConstraintKind::ForbidEdge {
                from: "web".into(),
                to: "services/api".into(),
                transitive: true,
                via: vec!["services/api/public".into()],
            }
        );
        assert_eq!(rules[0].min_tier.as_deref(), Some("fact"));
        assert_eq!(rules[1].kind, ConstraintKind::Invariant { text: "no routes".into(), rule: Some(rule.into()) });
        assert_eq!(rules[1].min_tier.as_deref(), Some("derived"));
        assert_eq!((rules[2].kind.name(), rules[2].min_tier.as_deref()), ("no_cycle", Some("fact")));
        // A version-1 entry reads with every version-2 field unset.
        let v1 = json!([{"source": "api", "id": "w", "kind": "forbid_edge", "from": "a", "to": "b"}]);
        let v1 = parse_constraints(&CellPayload::Json(v1.to_string()));
        assert_eq!(v1[0].kind, ConstraintKind::ForbidEdge { from: "a".into(), to: "b".into(), transitive: false, via: vec![] });
        assert_eq!(v1[0].min_tier, None);
        // Round trip through the writer's merge.
        let stored = merge_entry(None, &entries[0]).unwrap();
        let stored = merge_entry(Some(&stored), &entries[1]).unwrap();
        assert_eq!(parse_constraints(&stored), rules[..2]);
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
