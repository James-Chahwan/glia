//! CLI surface snapshot test slot (test-only; LG.6a fills it).
//!
//! Layout: one snapshot per top-level subcommand at `cli/surface/<command>.txt`
//! (kebab-case command name, e.g. `blast-radius.txt`), plus
//! `cli/surface/_global.txt` for `Cli`'s global options. No crate-name header
//! inside the files, so a crate rename touches only the snapshots of the
//! renamed binary. The packet that owns a command's module file regenerates
//! that command's snapshot in the same commit as the surface change.
