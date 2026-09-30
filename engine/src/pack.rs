//! Context packing to a token budget (CC.4b): seeds from `find`, a PPR
//! neighbourhood around them, a greedy value-per-token climb up the fidelity
//! ladder, a measured re-render loop that keeps the result inside the budget,
//! and a located, tiered manifest of what was packed and at which fidelity.
//! Public slot, reached by module path (`glia_engine::pack::<item>`). Filled by
//! CC.4b.
