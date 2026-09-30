//! Swift call scope (CB.18): implicit `self` and same-module static calls. A
//! bare call inside a type binds the type's own member first, across its
//! extensions in other files, and a static call or construction on a type
//! declared elsewhere in the same module binds that type. Crate-private: the
//! Swift builder runs its implicit-self pre-pass and consults its same-module
//! type hook. Filled by CB.18.
