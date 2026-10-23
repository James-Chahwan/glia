//! What a text says it is about (0.5.3 items 3, 6, 7 and 9): BM25 over past
//! commit messages (`text`), the repo's id vocabulary (`ids`), id definitions
//! read at a revision (`defs`), the path resolver (`paths`) and typed id
//! routing (`idr`). Query-time: reads the history snapshot's messages sidecar,
//! never runs inside a build. E0.2 declares every child here, so no owner
//! edits this file: EB.3 fills text.rs, EB.4 ids.rs, EB.5a / EB.5b defs.rs,
//! EB.5c paths.rs and EB.6 idr.rs.

pub mod defs;
pub mod idr;
pub mod ids;
pub mod paths;
pub mod text;
