//! The code domain's rules layer: `CodeFacts` (the `FactSource` over a
//! `MergedGraph`), `query` / `validate` / `query_gql`, the predicate prelude
//! and the reflexion model as rules, over glia-rules. Directory-module slot,
//! reached by module path (`glia_engine::rules::<item>`): its owners declare
//! their files here, namely facts.rs (DB.2a), query.rs (DB.2b, +DB.7c),
//! prelude.rs and prelude.dl (DB.2c), and reflexion.rs, reflexion.dl and the
//! `#[cfg(test)]` reflexion_parity.rs (DB.8a). Filled by DB.2a, DB.2b, DB.2c and
//! DB.8a.

/// CodeFacts, the code domain's FactSource over a MergedGraph (DB.2a). The module
/// is crate-visible so check reaches `rules::facts::{located_edge, tier_name,
/// edge_tier_of}` by path.
pub(crate) mod facts;

pub use facts::{CODE_ATTRIBUTES, CodeFacts};

/// `query` / `validate`: Datalog text over the code graph as a located, tiered
/// answer; `evaluate_rule` for check's rule invariants (DB.2b); `query_gql`, a GQL
/// read query over the same pipeline (DB.7c).
mod query;

pub use query::{
    QueryAnswer, QueryOptions, QueryOutput, QueryRow, QueryValue, RuleDiagnostic, RuleError,
    RuleRow, evaluate_rule, query, query_gql, validate,
};

/// The predicate prelude: structural predicates as rules (prelude.dl), merged into
/// every program and pruned to what it reads, and the engine relations they read
/// (DB.2c).
mod prelude;

pub use prelude::{PRELUDE, prelude_predicates};

/// The reflexion model as rules: reflexion.dl evaluated over check's model EDB and
/// read back into check's `ReflexionFacts` (DB.8a); `check_rules` derives the model
/// through it (DB.8b).
pub(crate) mod reflexion;

/// The reflexion goldens: seven fixtures' check reports, and a quokka probe, against
/// reports captured from the native derivation DB.8b deleted (DB.8a, DB.8b).
#[cfg(test)]
mod reflexion_parity;
