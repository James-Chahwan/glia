//! Communities (CD.1d): seeded Leiden over the code graph, scope-aware, with
//! structured per-community summaries (kinds, top members, entries, services,
//! effect sinks, inter-community links, cohesion) computed at query time,
//! located and tiered. The algorithm is `glia_activation::algo::community`;
//! this module answers with it. Public slot, reached by module path
//! (`glia_engine::communities::<item>`). Filled by CD.1d.
