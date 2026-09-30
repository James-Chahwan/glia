//! Communities (CD.1a, extended by CD.1b): a weighted undirected view of an
//! [`Adjacency`](crate::algo::Adjacency) with integer weights, seeded label
//! propagation and modularity (CD.1a), then seeded Leiden with its local
//! moving, refinement and aggregation, and the weighted quotient graph
//! (CD.1b). Domain-free: the categories and their weights come from the
//! caller or a domain's [`DomainTables`](crate::profile::DomainTables), never
//! a named code kind. Filled by CD.1a (+CD.1b).
