//! `glia watch` (DC.8): a foreground loop over `engine::watch::WatchSession`
//! printing one NDJSON object per update. The owner adds `Args`, `run`, the
//! `ChangeCmd::Watch` variant (its doc comment is the clap about) and its
//! match arm.
