//! `StoreError` — the one error type every store operation returns.

use repo_graph_core::NodeId;

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
}
