//! Service-split suggestions by minimum cut (CD.2b, + CD.2c): the module or
//! community quotient of a scope bisected by Stoer-Wagner (the ratio-best
//! phase cut above a balance floor, recursive to N parts), cut edges located
//! at their evidence sites and each part diffed against `glia arch`; CD.2c
//! adds the anchored s-t cut between two seeds or scopes and the blockers
//! (shared-write data entities, cycles between parts). The cuts are
//! `glia_activation::algo::cut`. Public slot, reached by module path
//! (`glia_engine::splits::<item>`). Filled by CD.2b.
