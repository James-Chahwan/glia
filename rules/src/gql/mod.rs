//! The GQL read subset (MATCH .. WHERE .. RETURN, aggregates, ORDER BY, LIMIT) parsed
//! over the shared lexer and lowered to the rule IR (Q4: set semantics, edge
//! homomorphism). DB.7a declares ast.rs and parser.rs here, DB.7b lower.rs; there is
//! no second lexer. Filled by DB.7a (+DB.7b).
