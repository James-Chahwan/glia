//! Timeline (CD.5a): edge and node validity intervals `[valid_from,
//! invalid_at)`, folded over N consecutive snapshots and their move maps
//! through [`delta`](crate::algo::delta) and keyed by each item's newest
//! identity. Domain-free: a snapshot is any
//! [`GraphSource`](crate::algo::GraphSource), never a named code kind. Filled
//! by CD.5a.
