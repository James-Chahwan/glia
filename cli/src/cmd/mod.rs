//! One module per command. Each `<command>.rs` holds a
//! `#[derive(clap::Args)] Args` with that command's fields and `#[arg]`s and a
//! `run(Args) -> i32`; `main.rs` keeps only the `Cmd` variant (which carries
//! the doc comment clap uses as `about`) and one dispatch arm.
//!
//! Commands added by the 0.5.0 leap go in an area directory instead —
//! `query`, `change`, `rules`, `store`, `inputs` — as one variant of that
//! area's flattened enum plus `<area>/<name>.rs`, so they list at top level
//! after `install-hooks` without touching `main.rs` or this file.
//!
//! Help text has one home: the variant's doc comment. `Args` structs and
//! area enums carry no `///` (probed on clap 4.6.1: one there does not change
//! `--help` while the variant has its own, so it would only mislead).

pub(crate) mod analyze;
pub(crate) mod arch;
pub(crate) mod blast_radius;
pub(crate) mod build;
pub(crate) mod contracts;
pub(crate) mod coverage;
pub(crate) mod docs;
pub(crate) mod docs_for;
pub(crate) mod impact;
pub(crate) mod merge;
pub(crate) mod projects;
pub(crate) mod resolve;
pub(crate) mod trace;

pub(crate) mod change;
pub(crate) mod inputs;
pub(crate) mod query;
pub(crate) mod rules;
pub(crate) mod store;
