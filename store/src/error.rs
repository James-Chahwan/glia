//! `StoreError` — the one error type every store operation returns.
//!
//! `needs_rebuild` / `rebuild_reason` split the variants into "the bytes on
//! disk are not a graph this build can read — regenerate it" and "the caller
//! asked for something that is not there". Consumers (the pyo3 loader, the MCP
//! wrapper) branch on `needs_rebuild` instead of matching variants, so a new
//! on-disk failure mode only has to be classified here.

use glia_core::NodeId;

use crate::container::FORMAT_VERSION;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("rkyv: {0}")]
    Rkyv(#[from] rkyv::rancor::Error),
    #[error("bad magic bytes — expected {expected:?}, got {got:?}")]
    BadMagic { expected: [u8; 4], got: [u8; 4] },
    #[error("unsupported format version {0} (this build supports {1})")]
    UnsupportedVersion(u32, u32),
    /// The file predates this build's format: no `GLIAGMAP` preamble at all
    /// (`found: None`, every file written by glia < 0.5.0), or a preamble
    /// carrying an older format version.
    #[error("{} - rebuild the graph", old_format_reason(*.found))]
    OldFormat { found: Option<u32> },
    /// The preamble names a format version newer than this build reads: the
    /// file was written by a newer glia.
    #[error("{} - rebuild the graph", future_format_reason(*.found))]
    FutureFormat { found: u32 },
    /// The preamble names this build's format version but the bytes do not
    /// hold a valid archive of it: a dev build from another commit (same
    /// number, different layout) or a damaged file.
    #[error("corrupt .gmap: {detail} - rebuild the graph")]
    Corrupt { detail: String },
    #[error("manifest json: {0}")]
    ManifestJson(#[from] serde_json::Error),
    #[error(
        "unsupported manifest schema version {got} (this build supports {supported})"
    )]
    ManifestSchemaVersion { got: u32, supported: u32 },
    #[error(
        "content hash mismatch on shard {shard} — manifest says {expected}, file is {got}"
    )]
    ContentHashMismatch {
        shard: String,
        expected: String,
        got: String,
    },
    #[error("manifest references shard {0} but the file is missing")]
    ShardMissing(String),
    #[error("node {0:?} not found in container")]
    NodeNotFound(NodeId),
    /// A write the caller got wrong (LF.1b `write_cell`): a cell type outside
    /// `WRITABLE`, a payload that does not fit its cell, an entry that breaks
    /// its rules, a sidecar with unreadable lines. Nothing was written.
    #[error("invalid cell write: {0}")]
    Invalid(String),
}

fn old_format_reason(found: Option<u32>) -> String {
    match found {
        None => "old format (no preamble, written by glia < 0.5.0)".to_string(),
        Some(v) => format!("old format v{v} (this build reads v{FORMAT_VERSION})"),
    }
}

fn future_format_reason(found: u32) -> String {
    format!("newer format v{found} (this build reads v{FORMAT_VERSION}; written by a newer glia)")
}

impl StoreError {
    /// True when the on-disk graph cannot be served by this build and the fix
    /// is to regenerate it: an old, future, foreign or damaged `.gmap`, a
    /// manifest of another schema, a shard that is missing or does not match
    /// its manifest hash, or no layout at all (`Io` NotFound). False for a
    /// caller error (`NodeNotFound`, `Invalid`) and for an I/O failure a
    /// rebuild would not fix (permissions, a full disk).
    pub fn needs_rebuild(&self) -> bool {
        match self {
            StoreError::Io(e) => e.kind() == std::io::ErrorKind::NotFound,
            StoreError::NodeNotFound(_) | StoreError::Invalid(_) => false,
            StoreError::Rkyv(_)
            | StoreError::BadMagic { .. }
            | StoreError::UnsupportedVersion(..)
            | StoreError::OldFormat { .. }
            | StoreError::FutureFormat { .. }
            | StoreError::Corrupt { .. }
            | StoreError::ManifestJson(_)
            | StoreError::ManifestSchemaVersion { .. }
            | StoreError::ContentHashMismatch { .. }
            | StoreError::ShardMissing(_) => true,
        }
    }

    /// A short human reason for the rebuild, for a log line or a wrapper's
    /// message; `None` exactly when `needs_rebuild` is false. Never carries
    /// rkyv / rancor internals.
    pub fn rebuild_reason(&self) -> Option<String> {
        if !self.needs_rebuild() {
            return None;
        }
        Some(match self {
            StoreError::OldFormat { found } => old_format_reason(*found),
            StoreError::FutureFormat { found } => future_format_reason(*found),
            StoreError::Corrupt { detail } => format!("corrupt: {detail}"),
            StoreError::BadMagic { got, .. } => format!("bad header magic {got:?}"),
            StoreError::UnsupportedVersion(got, supported) => {
                format!("header format v{got}, this build reads v{supported}")
            }
            StoreError::ManifestSchemaVersion { got, supported } => {
                format!("manifest schema {got}, this build reads {supported}")
            }
            StoreError::ManifestJson(e) => format!("manifest.json does not parse: {e}"),
            StoreError::ContentHashMismatch { shard, .. } => {
                format!("shard {shard} does not match its manifest hash")
            }
            StoreError::ShardMissing(shard) => format!("shard {shard} is missing"),
            StoreError::Rkyv(_) => "archive does not deserialize".to_string(),
            StoreError::Io(e) => format!("not found: {e}"),
            StoreError::NodeNotFound(_) | StoreError::Invalid(_) => return None,
        })
    }
}
