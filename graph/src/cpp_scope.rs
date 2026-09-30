//! C++ call scope (CB.25): a bare call inside a member binds the class's own
//! member first (implicit `this`), `Type::m()` / `ns::f()` bind through C++
//! name lookup, typed receivers resolve through `using` directives, and a call
//! through a header prototype binds the one external definition. Crate-private:
//! `build_c_cpp` runs its implicit-this pre-pass before `resolve_calls` and
//! consults its scope from the extra-hook after `CppCallScope`. Filled by CB.25.
