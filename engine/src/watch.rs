//! `WatchSession`: an incremental build held in memory, updated from changed
//! paths or a stat poll, persisted on demand. Public module slot, reached by
//! module path (`glia_engine::watch::WatchSession`). Filled by DC.7a (+DC.7b,
//! DC.7c).
