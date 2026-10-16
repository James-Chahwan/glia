//! The GQL read subset (MATCH .. WHERE .. RETURN, aggregates, ORDER BY, LIMIT) parsed
//! over the shared lexer and lowered to the rule IR (Q4: set semantics, edge
//! homomorphism). [`parse`] reads a query into the [`ast`] (DB.7a); [`compile`] parses
//! and lowers it to a rule program whose output is `result`, with the columns, ORDER BY
//! keys, LIMIT and path column the engine applies after evaluation (DB.7b). There is no
//! second lexer: the tokens are `crate::lexer`'s, the diagnostics `crate::diag`'s.

pub mod ast;
mod lower;
mod parser;

pub use lower::{Compiled, OrderKey, compile};
pub use parser::{SOURCE_NAME, parse};
