//! Rank fusion over per-candidate score vectors (0.5.3, slot E0.1, filled by
//! EB.2): the min-max normalisation over a candidate set, the weighted blends,
//! the equal means and the lexicographic order (LX) that every 0.5.3 text
//! answer and the per-repo calibration bank share. Domain-free: a caller hands
//! in one score per signal per candidate, indexed by candidate position, and
//! gets back an order over those positions; nothing here names a node kind,
//! an edge category or a file. Ties go by candidate index, so a caller's
//! index order is its tie order (never a NodeId, never map order).
