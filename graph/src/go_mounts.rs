//! Go route mounts (CB.20): route-mount prefixes resolved at build time. A
//! provisional mount ROUTE takes every prefix its parameter or field receives
//! through resolved calls (a fixpoint over the parser's mount facts), one ROUTE
//! per mount; an unmounted one keeps its local path. Crate-private:
//! `build_go_passes` binds them after `resolve_go_calls` and before the refs
//! are resolved. Filled by CB.20.
