//! Domain-free Datalog rule evaluator: a string-named rule IR, one lexer shared by
//! the Datalog surface and the GQL read subset, analysis, a planner, levelled
//! semi-naive evaluation over sorted batches, reaches / scc builtins over
//! activation::algo, provenance and explain, grouped aggregates and magic sets; its
//! fact seam is FactSource: GraphSource. Query-time: no stored derived edges. The
//! API is reached by module path: glia_rules::eval::evaluate,
//! glia_rules::parser::parse_program, glia_rules::source::FactSource. Every hash
//! map is value::Lookup (no iteration API); evaluation is single-threaded; output
//! rows sort by rendered values.

/// Name and type resolution, safety, arity, negation and aggregate checks (DA.5b).
pub mod analysis;
/// Spans, diagnostics and RulesError (DA.2).
pub mod diag;
/// evaluate / run: levelled semi-naive evaluation per stratum (DA.14b).
pub mod eval;
/// Evaluation::explain: a row's minimal-height derivation as a Witness (DA.15).
pub mod explain;
/// The string-named rule IR and the Catalog (DA.2).
pub mod ir;
/// The one tokenizer for the Datalog surface and the GQL read subset (DA.4).
pub mod lexer;
/// The Datalog surface parser into the IR (DA.9).
pub mod parser;
/// The FactSource: GraphSource trait, the Builtin trait and MemSource (DA.6).
pub mod source;
/// Deterministic MemSource graph builders for tests and gates (DA.13).
pub mod testkit;
/// The Tier lattice, Value / ValueRef, the ValueTable and Lookup (DA.3).
pub mod value;
/// The GQL read subset, parsed over the shared lexer and lowered to the IR (DB.7a, DB.7b).
pub mod gql;
/// Grouped count / min / max / sum as a stratum operator (DA.8).
mod aggregate;
/// The reaches / scc builtins over activation::algo (DA.11).
mod closure;
/// The step interpreter behind evaluation, aggregates and explain (DA.14a).
mod exec;
/// The magic-sets / demand rewrite for unseeded recursion (DA.12).
mod magic;
/// Per-stratum rule plans: semi-naive variants, bound-first order, index choice (DA.10).
mod plan;
/// Sorted, deduplicated batch storage with permuted key indexes (DA.7).
mod relation;
/// Stratification of the predicate dependency graph over activation::algo::cycles (DA.5a).
mod strata;
