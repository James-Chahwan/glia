//! Returns freed heap to the OS after a graph is dropped (glibc `malloc_trim`;
//! a no-op elsewhere): repeated builds in one process level off instead of
//! creeping. Public module slot, reached by module path
//! (`glia_engine::memory::<item>`). Filled by DE.10.
