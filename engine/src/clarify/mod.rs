//! The clarification queue (0.5.4, FC): questions the engine cannot settle from
//! the graph (an ambiguous key, a tie a rule cannot break), queued for an agent
//! or a human, and the recorded answers read back. Query-time, never a build
//! pass. F0.3 declares every child here, so no owner edits this file: FC.12
//! fills questions.rs and FC.13 answer.rs.

pub mod answer;
pub mod questions;
