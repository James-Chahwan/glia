//! Hubs (CD.4a): in / out degree per edge category in O(V + E) over an
//! [`Adjacency`](crate::algo::Adjacency), and fixed-iteration HITS hub and
//! authority scores in a deterministic order. Domain-free: the categories
//! arrive as a [`CategorySet`](crate::algo::CategorySet), never a named code
//! kind. Filled by CD.4a.
