//! The goal layer (0.5.4, FC): goal units (GOAL_UNIT nodes read from task
//! docs, tickets and PR text), the keys they cite, the resolvers that bind each
//! key to code, the tiered joiner that writes TRACES, and the answers over it
//! (why, impact, drift, check). Query-time annotation only: TRACES weighs 0 in
//! PPR and is in no carry or community list, and a GOAL_UNIT is no entry and no
//! find row, so the goal layer never feeds a walk or a ranking. F0.3 declares
//! every child here, so no owner edits this file: FC.3a fills extract.rs,
//! FC.5a resolve.rs, FC.5b join.rs, FC.7 why.rs, FC.8 impact.rs, FC.9 drift.rs
//! and FC.10 check.rs.

pub mod check;
pub mod drift;
pub mod extract;
pub mod impact;
pub mod join;
pub mod resolve;
pub mod why;
