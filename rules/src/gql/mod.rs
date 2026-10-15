//! The GQL read subset (MATCH .. WHERE .. RETURN, aggregates, ORDER BY, LIMIT) parsed
//! over the shared lexer and lowered to the rule IR (Q4: set semantics, edge
//! homomorphism). [`parse`] reads a query into the [`ast`] (DB.7a); DB.7b's lower.rs
//! lowers that tree to a rule program. There is no second lexer: the tokens are
//! `crate::lexer`'s, the diagnostics `crate::diag`'s.

pub mod ast;
mod parser;

pub use parser::{SOURCE_NAME, parse};
