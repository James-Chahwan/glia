//! Time-travel graph over N revs (CD.5c): build N first-parent revs (one
//! incremental build each on the shared parse cache, working-tree identity),
//! chain their deltas through LB.6's moves into intervals, write the sidecar,
//! and answer edge history and as-of views. The interval algebra is
//! `glia_activation::algo::timeline`. Public slot, reached by module path
//! (`glia_engine::timeline::<item>`). Filled by CD.5c.
