//! Identity classes: a union-find over NodeIds (min-id representative, path
//! compression) and a Quotient<G: GraphSource> adapter that maps every node to
//! its class representative, so a walk visits a class once. Filled by DD.1.
