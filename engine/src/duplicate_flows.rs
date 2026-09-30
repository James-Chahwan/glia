//! Duplicate entry flows (CD.4e): entry flows whose reached sets are
//! identical (exact, fingerprinted, DERIVED) or overlap at a Jaccard
//! threshold (MinHash / LSH candidates then verified, HEURISTIC), with utility
//! hubs and test entries left out. The sketches are
//! `glia_activation::algo::minhash`. Public slot, reached by module path
//! (`glia_engine::duplicate_flows::<item>`). Filled by CD.4e.
