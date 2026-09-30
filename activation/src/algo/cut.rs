//! Minimum cuts (CD.2a): the Stoer-Wagner global minimum cut, with every
//! phase cut, and the Dinic s-t maximum flow / minimum cut, both with integer
//! capacities over the weighted undirected view that
//! [`community`](crate::algo::community) builds. Domain-free: capacities come
//! from the caller's category weights, never a named code kind. Filled by
//! CD.2a.
