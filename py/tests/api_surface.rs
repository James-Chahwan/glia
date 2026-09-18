//! pyo3 API surface snapshot (LG.6a).
//!
//! Parses every `*.rs` under `py/src` with `syn` — the test binary never
//! links the cdylib, so the pyo3 link note in `src/lib.rs` does not apply —
//! and renders the Python-visible surface one snapshot per source module:
//! `py/api_surface/<module>.txt` (`lib.txt` holds the `#[pymodule]` name; a
//! nested `src/a/b.rs` is `a.b.txt`; a module with no surface has no file).
//!
//! Lines (sorted by subject, so a class sorts just before its members):
//!
//! ```text
//! pymodule <name>
//! fn <name>(<param>: <Rust type> [= <default>], ...) -> <Rust return type>
//! class <Name>[ (<#[pyclass] options>)]
//! method|staticmethod|classmethod|new <Class>.<name>(<params>) -> <return>
//! getter <Class>.<name> -> <type>        setter <Class>.<name>(<param>: <type>)
//! classattr <Class>.<name>: <type>       variant <Class>.<Variant>
//! ```
//!
//! Parameters are the Python-visible ones (receiver, `py: Python`, `slf` /
//! `cls` dropped); names and defaults come from `#[pyo3(signature = ...)]`
//! when present, types from the Rust parameter of that name. The return type
//! is kept because it shows JSON-string vs native (`PyResult<String>` vs
//! `Vec<u64>`). Docstrings are NOT pinned. `py/check_api_surface.py` checks
//! the same files against the installed wheel at wave close-out.
//!
//! Two assertions: (1) every `#[pyfunction]` / `#[pyclass]` is registered —
//! wrapped (`wrap_pyfunction!` / `add_class::<T>`) inside the `#[pymodule]`
//! fn or inside a fn submitted as `inventory::submit! { ModuleFns { add: f } }`
//! from the same file — every such registration names an existing one, and
//! no Python name is defined by two modules (with `multiple-pymethods` the
//! later one would silently win); (2) the rendered text equals the committed
//! snapshots. A packet that
//! changes the pyo3 surface regenerates the snapshots of the modules it
//! touches in the SAME commit:
//!
//! ```text
//! GLIA_UPDATE_SURFACE=1 cargo test --manifest-path py/Cargo.toml --test api_surface
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit::Visit;

const UPDATE_ENV: &str = "GLIA_UPDATE_SURFACE";
const REGENERATE: &str =
    "GLIA_UPDATE_SURFACE=1 cargo test --manifest-path py/Cargo.toml --test api_surface";

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// ---------------------------------------------------------------------------
// Token text: a compact, deterministic rendering of types and default values
// (`Vec<(u64, u32)>`, `&'static str`, `Option<&str>`), independent of how
// `TokenStream::to_string` happens to space things.

#[derive(Clone, Copy, PartialEq)]
enum Gap {
    /// Start of a stream, or after a token that binds to what follows.
    Tight,
    /// After an identifier, literal, closing group or `>`.
    Word,
    /// After `,` `;` `:` `=` `+` — the next token gets a space.
    Sep,
}

fn tokens_text(ts: TokenStream) -> String {
    let mut out = String::new();
    push_tokens(&mut out, ts);
    out
}

fn peek_is(iter: &mut std::iter::Peekable<proc_macro2::token_stream::IntoIter>, ch: char) -> bool {
    matches!(iter.peek(), Some(TokenTree::Punct(q)) if q.as_char() == ch)
}

fn push_tokens(out: &mut String, ts: TokenStream) {
    let mut gap = Gap::Tight;
    // The previous token was a `Joint` punct: this one glues onto it (`==`).
    let mut glue = false;
    let mut iter = ts.into_iter().peekable();
    while let Some(tt) = iter.next() {
        let was_glued = std::mem::replace(&mut glue, false);
        match tt {
            TokenTree::Group(g) => {
                let (open, close) = match g.delimiter() {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::Brace => ("{", "}"),
                    Delimiter::None => ("", ""),
                };
                if gap == Gap::Sep {
                    out.push(' ');
                }
                out.push_str(open);
                push_tokens(out, g.stream());
                out.push_str(close);
                gap = Gap::Word;
            }
            TokenTree::Ident(_) | TokenTree::Literal(_) => {
                if gap != Gap::Tight {
                    out.push(' ');
                }
                out.push_str(&tt.to_string());
                gap = Gap::Word;
            }
            TokenTree::Punct(p) => {
                let c = p.as_char();
                let joint = p.spacing() == Spacing::Joint;
                match c {
                    ':' if joint && peek_is(&mut iter, ':') => {
                        iter.next();
                        out.push_str("::");
                        gap = Gap::Tight;
                    }
                    '-' if joint && peek_is(&mut iter, '>') => {
                        iter.next();
                        out.push_str(" -> ");
                        gap = Gap::Tight;
                    }
                    ',' | ';' | ':' => {
                        out.push(c);
                        gap = Gap::Sep;
                    }
                    '<' => {
                        out.push(c);
                        gap = Gap::Tight;
                    }
                    '>' => {
                        out.push(c);
                        gap = Gap::Word;
                    }
                    '.' | '!' => {
                        out.push(c);
                        gap = Gap::Tight;
                    }
                    // Prefix operators: `&T`, `*const T`, `'a`, unary `-1`.
                    '&' | '*' | '\'' | '#' | '?' | '~' => {
                        if gap != Gap::Tight {
                            out.push(' ');
                        }
                        out.push(c);
                        gap = Gap::Tight;
                    }
                    '-' if gap != Gap::Word => {
                        if gap == Gap::Sep {
                            out.push(' ');
                        }
                        out.push(c);
                        gap = Gap::Tight;
                    }
                    // Binary operators (`=`, `+`, `-`, `|`, …): spaced.
                    _ => {
                        if !was_glued && !out.is_empty() && !out.ends_with(' ') {
                            out.push(' ');
                        }
                        out.push(c);
                        gap = if joint { Gap::Tight } else { Gap::Sep };
                    }
                }
                glue = joint;
            }
        }
    }
}

fn type_text(ty: &syn::Type) -> String {
    tokens_text(ty.to_token_stream())
}

fn return_text(ret: &syn::ReturnType) -> String {
    match ret {
        syn::ReturnType::Default => "()".to_string(),
        syn::ReturnType::Type(_, ty) => type_text(ty),
    }
}

/// Split a token stream at its top-level commas. A `(..)` / `[..]` group is one
/// token tree, so its commas never split; generic `<..>` is NOT a group, so
/// angle depth is tracked to keep `Map<A, B>` whole.
fn split_top_level_commas(ts: TokenStream) -> Vec<TokenStream> {
    let mut parts = Vec::new();
    let mut cur: Vec<TokenTree> = Vec::new();
    let mut angle = 0i32;
    for tt in ts {
        if let TokenTree::Punct(p) = &tt {
            match p.as_char() {
                '<' => angle += 1,
                '>' if angle > 0 => angle -= 1,
                ',' if angle == 0 => {
                    parts.push(cur.drain(..).collect());
                    continue;
                }
                _ => {}
            }
        }
        cur.push(tt);
    }
    if !cur.is_empty() {
        parts.push(cur.into_iter().collect());
    }
    parts
}

// ---------------------------------------------------------------------------
// Attributes.

fn attr_is(attr: &syn::Attribute, name: &str) -> bool {
    attr.path().segments.last().is_some_and(|s| s.ident == name)
}

fn find_attr<'a>(attrs: &'a [syn::Attribute], name: &str) -> Option<&'a syn::Attribute> {
    attrs.iter().find(|a| attr_is(a, name))
}

fn attr_args(attr: &syn::Attribute) -> Option<TokenStream> {
    match &attr.meta {
        syn::Meta::List(list) => Some(list.tokens.clone()),
        _ => None,
    }
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        attr_is(a, "cfg")
            && attr_args(a).is_some_and(|t| {
                t.into_iter()
                    .any(|tt| matches!(tt, TokenTree::Ident(i) if i == "test"))
            })
    })
}

/// pyo3 options from `#[pyo3(...)]` plus the options list of `#[<primary>(...)]`
/// (`#[pyfunction(name = "x")]`, `#[pyclass(frozen)]`): `key -> value tokens`
/// (a bare flag maps to an empty stream).
fn pyo3_options(attrs: &[syn::Attribute], primary: &str) -> Vec<(String, TokenStream)> {
    let mut out = Vec::new();
    for attr in attrs
        .iter()
        .filter(|a| attr_is(a, "pyo3") || attr_is(a, primary))
    {
        let Some(args) = attr_args(attr) else {
            continue;
        };
        for part in split_top_level_commas(args) {
            let mut it = part.into_iter();
            let Some(TokenTree::Ident(key)) = it.next() else {
                continue;
            };
            let rest: Vec<TokenTree> = it.collect();
            let value = match rest.split_first() {
                Some((TokenTree::Punct(eq), tail)) if eq.as_char() == '=' => {
                    tail.iter().cloned().collect()
                }
                _ => rest.into_iter().collect(),
            };
            out.push((key.to_string(), value));
        }
    }
    out
}

fn option<'a>(opts: &'a [(String, TokenStream)], key: &str) -> Option<&'a TokenStream> {
    opts.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn string_value(ts: &TokenStream) -> Option<String> {
    let lit: syn::LitStr = syn::parse2(ts.clone()).ok()?;
    Some(lit.value())
}

fn has_flag(opts: &[(String, TokenStream)], key: &str) -> bool {
    opts.iter().any(|(k, _)| k == key)
}

// ---------------------------------------------------------------------------
// Signatures.

fn type_last_ident(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        syn::Type::Reference(r) => type_last_ident(&r.elem),
        _ => None,
    }
}

/// `PyRef<'_, Self>`, `&Bound<'_, Self>`, `Py<Self>`: `Self` among the type's
/// top-level tokens (generic arguments are not groups).
fn mentions_self(ty: &syn::Type) -> bool {
    ty.to_token_stream()
        .into_iter()
        .any(|tt| matches!(tt, TokenTree::Ident(i) if i == "Self"))
}

/// The Rust parameters Python sees, in order: receivers, `Python<'_>` tokens,
/// a `slf: PyRef<Self>`-style receiver (only when there is no `self`) and a
/// classmethod's `cls` are dropped.
fn python_params(sig: &syn::Signature, drop_first_typed: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut first_typed = sig.receiver().is_none();
    for input in &sig.inputs {
        let syn::FnArg::Typed(pt) = input else {
            continue;
        };
        let is_first = std::mem::replace(&mut first_typed, false);
        if type_last_ident(&pt.ty).as_deref() == Some("Python") {
            continue;
        }
        if is_first && (drop_first_typed || mentions_self(&pt.ty)) {
            continue;
        }
        let name = match &*pt.pat {
            syn::Pat::Ident(pi) => pi.ident.to_string().trim_start_matches("r#").to_string(),
            other => tokens_text(other.to_token_stream()),
        };
        out.push((name, type_text(&pt.ty)));
    }
    out
}

/// `(<params>)` — from `#[pyo3(signature = (...))]` when present (names,
/// order, defaults, `*` / `/` markers), else from the Rust parameters.
fn render_params(sig: &syn::Signature, opts: &[(String, TokenStream)], drop_first: bool) -> String {
    let rust = python_params(sig, drop_first);
    let type_of = |name: &str| rust.iter().find(|(n, _)| n == name).map(|(_, t)| t.clone());
    let mut parts = Vec::new();
    let declared = option(opts, "signature").and_then(|ts| match ts.clone().into_iter().next() {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => Some(g.stream()),
        _ => None,
    });
    match declared {
        Some(items) => {
            for item in split_top_level_commas(items) {
                let toks: Vec<TokenTree> = item.into_iter().collect();
                let eq = toks
                    .iter()
                    .position(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == '='));
                let (head, default) = match eq {
                    Some(i) => (
                        &toks[..i],
                        Some(tokens_text(toks[i + 1..].iter().cloned().collect())),
                    ),
                    None => (&toks[..], None),
                };
                let head_text = tokens_text(head.iter().cloned().collect());
                let name = head_text.trim_start_matches('*');
                let mut part = head_text.clone();
                if !name.is_empty() && name != "/" {
                    if let Some(t) = type_of(name) {
                        let _ = write!(part, ": {t}");
                    }
                }
                if let Some(d) = default {
                    let _ = write!(part, " = {d}");
                }
                parts.push(part);
            }
        }
        None => {
            for (name, ty) in rust {
                parts.push(format!("{name}: {ty}"));
            }
        }
    }
    format!("({})", parts.join(", "))
}

fn python_name(ident: &syn::Ident, opts: &[(String, TokenStream)]) -> String {
    option(opts, "name")
        .and_then(string_value)
        .unwrap_or_else(|| ident.to_string().trim_start_matches("r#").to_string())
}

// ---------------------------------------------------------------------------
// Collection.

/// One class member. `owner` is the Rust type ident: the `#[pyclass]` that
/// names it for Python may live in another file, so the class name is looked
/// up at render time.
struct Member {
    module: String,
    owner: String,
    kind: &'static str,
    name: String,
    /// What follows the name: `(<params>) -> <ret>`, ` -> <type>`, `: <type>`.
    tail: String,
}

#[derive(Default)]
struct Surface {
    /// Snapshot stem → module-level lines (`pymodule`, `fn`, `class`).
    lines: BTreeMap<String, BTreeSet<String>>,
    members: Vec<Member>,
    /// `(python name, module)` of every `#[pyfunction]`.
    fn_names: Vec<(String, String)>,
    /// Snapshot stem → source path relative to the crate (`src/graph.rs`).
    module_files: BTreeMap<String, String>,
    /// `(module, rust ident)` of every `#[pyfunction]`.
    pyfunctions: BTreeSet<(String, String)>,
    /// `(module, rust ident)` of every `#[pyclass]`.
    pyclasses: BTreeSet<(String, String)>,
    /// Rust type ident → Python class name.
    class_names: BTreeMap<String, String>,
    /// `(module, type ident)` of every `#[pymethods]` block.
    pymethod_owners: Vec<(String, String)>,
    /// `(module, fn ident)` → paths passed to `wrap_pyfunction!` / `add_class`.
    wraps: BTreeMap<(String, String), Vec<Vec<String>>>,
    classes_added: BTreeMap<(String, String), Vec<Vec<String>>>,
    /// Fns that run at module init: the `#[pymodule]` fn(s), and every
    /// `add: f` named in an `inventory::submit!` in the same file.
    live: BTreeSet<(String, String)>,
    unsupported: Vec<String>,
}

struct FileVisitor<'a> {
    module: String,
    surface: &'a mut Surface,
    fn_stack: Vec<String>,
}

impl FileVisitor<'_> {
    fn add_line(&mut self, line: String) {
        self.surface
            .lines
            .entry(self.module.clone())
            .or_default()
            .insert(line);
    }

    fn add_member(&mut self, owner: &str, kind: &'static str, name: String, tail: String) {
        self.surface.members.push(Member {
            module: self.module.clone(),
            owner: owner.to_string(),
            kind,
            name,
            tail,
        });
    }

    fn record_pyfunction(&mut self, f: &syn::ItemFn) {
        let opts = pyo3_options(&f.attrs, "pyfunction");
        let name = python_name(&f.sig.ident, &opts);
        let params = render_params(&f.sig, &opts, false);
        let ret = return_text(&f.sig.output);
        self.add_line(format!("fn {name}{params} -> {ret}"));
        self.surface.fn_names.push((name, self.module.clone()));
        self.surface
            .pyfunctions
            .insert((self.module.clone(), f.sig.ident.to_string()));
    }

    fn record_class(
        &mut self,
        ident: &syn::Ident,
        attrs: &[syn::Attribute],
    ) -> Vec<(String, TokenStream)> {
        let opts = pyo3_options(attrs, "pyclass");
        let name = python_name(ident, &opts);
        let raw = find_attr(attrs, "pyclass")
            .and_then(attr_args)
            .map(tokens_text)
            .unwrap_or_default();
        let line = if raw.is_empty() {
            format!("class {name}")
        } else {
            format!("class {name} ({raw})")
        };
        self.add_line(line);
        self.surface
            .pyclasses
            .insert((self.module.clone(), ident.to_string()));
        self.surface.class_names.insert(ident.to_string(), name);
        opts
    }

    fn record_method(&mut self, owner: &str, f: &syn::ImplItemFn) {
        let attrs = &f.attrs;
        let opts = pyo3_options(attrs, "pyo3");
        let ident = f.sig.ident.to_string();
        let ret = return_text(&f.sig.output);
        let accessor_name = |attr: &str, prefix: &str| -> String {
            find_attr(attrs, attr)
                .and_then(attr_args)
                .map(tokens_text)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_matches('"').to_string())
                .or_else(|| option(&opts, "name").and_then(string_value))
                .unwrap_or_else(|| ident.strip_prefix(prefix).unwrap_or(&ident).to_string())
        };
        let (kind, name, tail) = if find_attr(attrs, "getter").is_some() {
            (
                "getter",
                accessor_name("getter", "get_"),
                format!(" -> {ret}"),
            )
        } else if find_attr(attrs, "setter").is_some() {
            let params = render_params(&f.sig, &opts, false);
            ("setter", accessor_name("setter", "set_"), params)
        } else if find_attr(attrs, "classattr").is_some() {
            let name = python_name(&f.sig.ident, &opts);
            ("classattr", name, format!(": {ret}"))
        } else {
            let (kind, drop_first) = if find_attr(attrs, "new").is_some() {
                ("new", false)
            } else if find_attr(attrs, "staticmethod").is_some() {
                ("staticmethod", false)
            } else if find_attr(attrs, "classmethod").is_some() {
                ("classmethod", true)
            } else {
                ("method", false)
            };
            let name = if kind == "new" {
                "__new__".to_string()
            } else {
                python_name(&f.sig.ident, &opts)
            };
            let params = render_params(&f.sig, &opts, drop_first);
            (kind, name, format!("{params} -> {ret}"))
        };
        self.add_member(owner, kind, name, tail);
    }

    fn scan_registrations(&mut self, mac: &syn::Macro) {
        let Some(owner) = self.fn_stack.last().cloned() else {
            return;
        };
        if mac
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident == "wrap_pyfunction")
        {
            let first = split_top_level_commas(mac.tokens.clone())
                .into_iter()
                .next();
            if let Some(path) = first.and_then(|t| syn::parse2::<syn::Path>(t).ok()) {
                let segs = path.segments.iter().map(|s| s.ident.to_string()).collect();
                self.surface
                    .wraps
                    .entry((self.module.clone(), owner))
                    .or_default()
                    .push(segs);
            }
        }
    }
}

impl<'ast> Visit<'ast> for FileVisitor<'_> {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        if is_cfg_test(&m.attrs) {
            return;
        }
        if find_attr(&m.attrs, "pymodule").is_some() {
            self.surface.unsupported.push(format!(
                "{}: declarative `#[pymodule] mod {}` is not modelled by api_surface.rs — extend it",
                self.module, m.ident
            ));
        }
        syn::visit::visit_item_mod(self, m);
    }

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if is_cfg_test(&f.attrs) {
            return;
        }
        if find_attr(&f.attrs, "pyfunction").is_some() {
            self.record_pyfunction(f);
        }
        if find_attr(&f.attrs, "pymodule").is_some() {
            let opts = pyo3_options(&f.attrs, "pymodule");
            let name = python_name(&f.sig.ident, &opts);
            self.add_line(format!("pymodule {name}"));
            self.surface
                .live
                .insert((self.module.clone(), f.sig.ident.to_string()));
        }
        self.fn_stack.push(f.sig.ident.to_string());
        syn::visit::visit_item_fn(self, f);
        self.fn_stack.pop();
    }

    fn visit_item_struct(&mut self, s: &'ast syn::ItemStruct) {
        if find_attr(&s.attrs, "pyclass").is_some() && !is_cfg_test(&s.attrs) {
            let opts = self.record_class(&s.ident, &s.attrs);
            let owner = s.ident.to_string();
            let get_all = has_flag(&opts, "get_all");
            let set_all = has_flag(&opts, "set_all");
            if let syn::Fields::Named(named) = &s.fields {
                for field in &named.named {
                    let Some(ident) = &field.ident else { continue };
                    let fopts = pyo3_options(&field.attrs, "pyo3");
                    let name = python_name(ident, &fopts);
                    let ty = type_text(&field.ty);
                    if get_all || has_flag(&fopts, "get") {
                        self.add_member(&owner, "getter", name.clone(), format!(" -> {ty}"));
                    }
                    if set_all || has_flag(&fopts, "set") {
                        self.add_member(&owner, "setter", name, format!("(value: {ty})"));
                    }
                }
            }
        }
        syn::visit::visit_item_struct(self, s);
    }

    fn visit_item_enum(&mut self, e: &'ast syn::ItemEnum) {
        if find_attr(&e.attrs, "pyclass").is_some() && !is_cfg_test(&e.attrs) {
            self.record_class(&e.ident, &e.attrs);
            let owner = e.ident.to_string();
            for v in &e.variants {
                let vopts = pyo3_options(&v.attrs, "pyo3");
                let name = python_name(&v.ident, &vopts);
                self.add_member(&owner, "variant", name, String::new());
            }
        }
        syn::visit::visit_item_enum(self, e);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if is_cfg_test(&i.attrs) {
            return;
        }
        if find_attr(&i.attrs, "pymethods").is_some() {
            let owner = type_last_ident(&i.self_ty).unwrap_or_default();
            self.surface
                .pymethod_owners
                .push((self.module.clone(), owner.clone()));
            for item in &i.items {
                match item {
                    syn::ImplItem::Fn(f) => self.record_method(&owner, f),
                    syn::ImplItem::Const(c) if find_attr(&c.attrs, "classattr").is_some() => {
                        let opts = pyo3_options(&c.attrs, "pyo3");
                        let name = python_name(&c.ident, &opts);
                        let tail = format!(": {}", type_text(&c.ty));
                        self.add_member(&owner, "classattr", name, tail);
                    }
                    _ => {}
                }
            }
        }
        syn::visit::visit_item_impl(self, i);
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.fn_stack.push(f.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, f);
        self.fn_stack.pop();
    }

    fn visit_item_macro(&mut self, m: &'ast syn::ItemMacro) {
        if m.mac
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident == "submit")
        {
            let mut found = Vec::new();
            find_add_fields(m.mac.tokens.clone(), &mut found);
            for path in found {
                self.surface.live.insert(resolve(&path, &self.module));
            }
        }
        syn::visit::visit_item_macro(self, m);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.scan_registrations(mac);
        syn::visit::visit_macro(self, mac);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if call.method == "add_class" {
            let ty = call.turbofish.as_ref().and_then(|t| t.args.first());
            if let (Some(syn::GenericArgument::Type(syn::Type::Path(p))), Some(owner)) =
                (ty, self.fn_stack.last().cloned())
            {
                let segs = p
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                self.surface
                    .classes_added
                    .entry((self.module.clone(), owner))
                    .or_default()
                    .push(segs);
            }
        }
        syn::visit::visit_expr_method_call(self, call);
    }
}

/// The path in every `add: <path>` field of a `ModuleFns { .. }` literal, at
/// any depth.
fn find_add_fields(ts: TokenStream, out: &mut Vec<Vec<String>>) {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    for (i, tt) in toks.iter().enumerate() {
        match tt {
            TokenTree::Group(g) => find_add_fields(g.stream(), out),
            TokenTree::Ident(id) if id == "add" => {
                let Some(TokenTree::Punct(colon)) = toks.get(i + 1) else {
                    continue;
                };
                if colon.as_char() != ':' || colon.spacing() != Spacing::Alone {
                    continue;
                }
                let value: TokenStream = toks[i + 2..]
                    .iter()
                    .take_while(|t| !matches!(t, TokenTree::Punct(p) if p.as_char() == ','))
                    .cloned()
                    .collect();
                if let Ok(path) = syn::parse2::<syn::Path>(value) {
                    out.push(path.segments.iter().map(|s| s.ident.to_string()).collect());
                }
            }
            _ => {}
        }
    }
}

/// `src/graph.rs` → `graph`, `src/a/mod.rs` → `a`, `src/a/b.rs` → `a.b`.
fn module_stem(src: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(src).unwrap_or(file).with_extension("");
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.len() > 1 && parts.last().is_some_and(|p| p == "mod") {
        parts.pop();
    }
    parts.join(".")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn collect(src: &Path) -> Surface {
    let mut files = Vec::new();
    rust_files(src, &mut files);
    files.sort();
    let mut surface = Surface::default();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        let ast =
            syn::parse_file(&text).unwrap_or_else(|e| panic!("parse {}: {e}", file.display()));
        let module = module_stem(src, &file);
        let rel = file
            .strip_prefix(src)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        surface
            .module_files
            .insert(module.clone(), format!("src/{rel}"));
        let mut v = FileVisitor {
            module,
            surface: &mut surface,
            fn_stack: Vec::new(),
        };
        v.visit_file(&ast);
    }
    surface
}

fn class_name<'a>(s: &'a Surface, owner: &'a str) -> &'a str {
    s.class_names.get(owner).map_or(owner, String::as_str)
}

/// Resolve a registration path against `(module, ident)` definitions: a bare
/// ident is looked up in the registering file; `crate::a::f` / `a::f` in `a`.
fn resolve(path: &[String], registrar_module: &str) -> (String, String) {
    let Some((last, prefix)) = path.split_last() else {
        return (registrar_module.to_string(), String::new());
    };
    let prefix: Vec<&str> = prefix
        .iter()
        .map(String::as_str)
        .filter(|s| *s != "crate" && *s != "self")
        .collect();
    let module = if prefix.is_empty() {
        registrar_module.to_string()
    } else {
        prefix.join(".")
    };
    (module, last.clone())
}

/// Registration problems, empty when every definition is registered exactly
/// against something that exists.
fn registration_report(s: &Surface) -> String {
    let mut registered_fns = BTreeSet::new();
    let mut registered_classes = BTreeSet::new();
    let mut dangling = Vec::new();
    let mut dead = BTreeSet::new();
    for (key, paths) in &s.wraps {
        if !s.live.contains(key) {
            dead.insert(format!("{}::{}", key.0, key.1));
            continue;
        }
        for p in paths {
            let target = resolve(p, &key.0);
            if s.pyfunctions.contains(&target) {
                registered_fns.insert(target);
            } else {
                dangling.push(format!("wrap_pyfunction!({})", p.join("::")));
            }
        }
    }
    for (key, paths) in &s.classes_added {
        if !s.live.contains(key) {
            dead.insert(format!("{}::{}", key.0, key.1));
            continue;
        }
        for p in paths {
            let target = resolve(p, &key.0);
            if s.pyclasses.contains(&target) {
                registered_classes.insert(target);
            } else {
                dangling.push(format!("add_class::<{}>", p.join("::")));
            }
        }
    }
    let unregistered: Vec<String> = s
        .pyfunctions
        .iter()
        .filter(|k| !registered_fns.contains(*k))
        .map(|(_, f)| f.clone())
        .collect();
    let unregistered_classes: Vec<String> = s
        .pyclasses
        .iter()
        .filter(|k| !registered_classes.contains(*k))
        .map(|(_, c)| c.clone())
        .collect();
    let orphans: Vec<String> = s
        .pymethod_owners
        .iter()
        .filter(|(_, owner)| !s.pyclasses.iter().any(|(_, c)| c == owner))
        .map(|(m, o)| format!("{m}: impl {o}"))
        .collect();

    let mut report = String::new();
    if !unregistered.is_empty() {
        let _ = writeln!(report, "unregistered: [{}]", unregistered.join(", "));
    }
    if !unregistered_classes.is_empty() {
        let _ = writeln!(
            report,
            "unregistered classes: [{}]",
            unregistered_classes.join(", ")
        );
    }
    if !dangling.is_empty() {
        let _ = writeln!(report, "dangling: [{}]", dangling.join(", "));
    }
    if !dead.is_empty() {
        let dead: Vec<String> = dead.into_iter().collect();
        let _ = writeln!(
            report,
            "never run at module init (no `#[pymodule]`, no `inventory::submit!` add): [{}]",
            dead.join(", ")
        );
    }
    if !orphans.is_empty() {
        let _ = writeln!(
            report,
            "#[pymethods] on a non-#[pyclass] type: [{}]",
            orphans.join(", ")
        );
    }
    // Two modules defining the same Python name: pyo3 registers both and the
    // later one silently wins (`multiple-pymethods` / `ModuleFns` order).
    let mut seen: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (name, module) in &s.fn_names {
        seen.entry(format!("fn {name}")).or_default().push(module);
    }
    for m in &s.members {
        let slot = if m.kind == "setter" {
            "setter"
        } else {
            "member"
        };
        let key = format!("{slot} {}.{}", class_name(s, &m.owner), m.name);
        seen.entry(key).or_default().push(&m.module);
    }
    for (what, modules) in seen.iter().filter(|(_, m)| m.len() > 1) {
        let _ = writeln!(report, "duplicate {what} in [{}]", modules.join(", "));
    }
    for u in &s.unsupported {
        let _ = writeln!(report, "{u}");
    }
    report
}

fn subject(line: &str) -> &str {
    line.split_once(' ').map_or(line, |(_, rest)| rest)
}

fn render(s: &Surface) -> BTreeMap<String, String> {
    let mut by_module = s.lines.clone();
    for m in &s.members {
        let line = format!(
            "{} {}.{}{}",
            m.kind,
            class_name(s, &m.owner),
            m.name,
            m.tail
        );
        by_module.entry(m.module.clone()).or_default().insert(line);
    }
    by_module
        .iter()
        .map(|(module, lines)| {
            let mut sorted: Vec<&String> = lines.iter().collect();
            sorted.sort_by(|a, b| subject(a).cmp(subject(b)).then(a.cmp(b)));
            let file = s.module_files.get(module).cloned().unwrap_or_default();
            let mut body = format!(
                "# pyo3 surface of py/{file}. Regenerate in the commit that changes it: {REGENERATE}\n"
            );
            for l in sorted {
                body.push_str(l);
                body.push('\n');
            }
            (module.clone(), body)
        })
        .collect()
}

/// Changed lines only, `-` for the committed snapshot and `+` for the source,
/// in file order (an LCS walk; the files are a few dozen lines).
fn line_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = String::new();
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            let _ = writeln!(out, "-{}", a[i]);
            i += 1;
        } else {
            let _ = writeln!(out, "+{}", b[j]);
            j += 1;
        }
    }
    out
}

fn read_snapshots(dir: &Path) -> BTreeMap<String, String> {
    let mut on_disk = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return on_disk;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            on_disk.insert(
                stem.to_string(),
                std::fs::read_to_string(&path).unwrap_or_default(),
            );
        }
    }
    on_disk
}

#[test]
fn api_surface_registrations_are_complete() {
    let surface = collect(&crate_dir().join("src"));
    let report = registration_report(&surface);
    assert!(
        report.is_empty(),
        "pyo3 registration check failed:\n{report}"
    );
}

#[test]
fn api_surface_matches_snapshot() {
    let dir = crate_dir().join("api_surface");
    let rendered = render(&collect(&crate_dir().join("src")));
    let on_disk = read_snapshots(&dir);

    if std::env::var(UPDATE_ENV).as_deref() == Ok("1") {
        std::fs::create_dir_all(&dir).expect("create py/api_surface");
        for stem in on_disk.keys().filter(|s| !rendered.contains_key(*s)) {
            std::fs::remove_file(dir.join(format!("{stem}.txt"))).expect("remove stale snapshot");
        }
        for (stem, body) in &rendered {
            if on_disk.get(stem) != Some(body) {
                std::fs::write(dir.join(format!("{stem}.txt")), body).expect("write snapshot");
            }
        }
        return;
    }

    let mut report = String::new();
    for (stem, body) in &rendered {
        match on_disk.get(stem) {
            None => {
                let _ = writeln!(
                    report,
                    "py/api_surface/{stem}.txt: missing\n{}",
                    line_diff("", body)
                );
            }
            Some(committed) if committed != body => {
                let _ = writeln!(
                    report,
                    "py/api_surface/{stem}.txt: differs\n{}",
                    line_diff(committed, body)
                );
            }
            Some(_) => {}
        }
    }
    for stem in on_disk.keys().filter(|s| !rendered.contains_key(*s)) {
        let _ = writeln!(
            report,
            "py/api_surface/{stem}.txt: stale (module has no pyo3 surface)"
        );
    }
    assert!(
        report.is_empty(),
        "pyo3 API surface changed. If intended, regenerate in the same commit:\n  {REGENERATE}\n\n{report}"
    );
}

// ---------------------------------------------------------------------------
// The collector itself, on inline sources (so a regression in the scanner
// cannot hide behind an equally-wrong committed snapshot).

/// `tag` keeps the temp roots of tests running in parallel apart.
fn collect_inline(tag: &str, files: &[(&str, &str)]) -> Surface {
    let root = std::env::temp_dir().join(format!("glia-api-surface-{}-{tag}", std::process::id()));
    let src = root.join("src");
    for (name, body) in files {
        let path = src.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("temp dir");
        }
        std::fs::write(&path, body).expect("write temp source");
    }
    let s = collect(&src);
    let _ = std::fs::remove_dir_all(&root);
    s
}

const INLINE_LIB: &str = r#"
    #[pymodule]
    fn demo(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add_class::<a::Thing>()?;
        Ok(())
    }
"#;

#[test]
fn api_surface_renders_signatures_defaults_and_kinds() {
    let a = r#"
        #[pyclass(name = "Widget")]
        pub struct Thing { #[pyo3(get)] size: u32, hidden: u8 }

        #[pymethods]
        impl Thing {
            #[getter]
            fn get_label(&self) -> String { String::new() }
            #[pyo3(signature = (qname, depth=6, top_k=None, *, flag=false))]
            fn walk(&self, py: Python<'_>, qname: &str, depth: usize, top_k: Option<usize>, flag: bool) -> PyResult<String> { todo!() }
            #[staticmethod]
            fn make(n: i64) -> Vec<(u64, u32)> { Vec::new() }
        }

        #[pyfunction]
        #[pyo3(name = "renamed")]
        fn original(_py: Python<'_>, s: &'static str) -> &'static str { s }

        fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
            m.add_function(wrap_pyfunction!(original, m)?)?;
            Ok(())
        }
        inventory::submit! { ModuleFns { name: "a", add: register } }

        #[cfg(test)]
        mod tests { #[pyfunction] fn only_in_tests() {} }
    "#;
    let s = collect_inline("render", &[("lib.rs", INLINE_LIB), ("a.rs", a)]);
    let rendered = render(&s);
    let body: Vec<&str> = rendered["a"].lines().skip(1).collect();
    assert_eq!(
        body,
        [
            "class Widget (name = \"Widget\")",
            "getter Widget.label -> String",
            "staticmethod Widget.make(n: i64) -> Vec<(u64, u32)>",
            "getter Widget.size -> u32",
            "method Widget.walk(qname: &str, depth: usize = 6, top_k: Option<usize> = None, *, flag: bool = false) -> PyResult<String>",
            "fn renamed(s: &'static str) -> &'static str",
        ]
    );
    assert_eq!(rendered["lib"].lines().nth(1), Some("pymodule demo"));
    assert_eq!(registration_report(&s), "");
}

#[test]
fn api_surface_reports_unregistered_and_dangling() {
    let a = r#"
        #[pyclass] pub struct Thing {}
        #[pyfunction] fn kept() {}
        #[pyfunction] fn forgotten() {}
        #[pyfunction] fn orphaned_registrar() {}
        fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
            m.add_function(wrap_pyfunction!(kept, m)?)?;
            m.add_function(wrap_pyfunction!(gone, m)?)?;
            Ok(())
        }
        fn never_submitted(m: &Bound<'_, PyModule>) -> PyResult<()> {
            m.add_function(wrap_pyfunction!(orphaned_registrar, m)?)?;
            Ok(())
        }
        inventory::submit! { ModuleFns { name: "a", add: register } }
    "#;
    let b = r#"
        #[pymethods] impl Thing { fn twice(&self) {} }
        #[pymethods] impl Stranger { fn lost(&self) {} }
    "#;
    let c = r#"#[pymethods] impl Thing { fn twice(&self) {} }"#;
    let s = collect_inline(
        "report",
        &[
            ("lib.rs", INLINE_LIB),
            ("a.rs", a),
            ("b.rs", b),
            ("c.rs", c),
        ],
    );
    let report = registration_report(&s);
    assert!(
        report.contains("duplicate member Thing.twice in [b, c]\n"),
        "{report}"
    );
    assert!(report.contains("[b: impl Stranger]"), "{report}");
    assert!(
        report.contains("unregistered: [forgotten, orphaned_registrar]\n"),
        "{report}"
    );
    assert!(
        report.contains("dangling: [wrap_pyfunction!(gone)]\n"),
        "{report}"
    );
    assert!(report.contains("[a::never_submitted]"), "{report}");
}

#[test]
fn api_surface_line_diff_reports_only_changed_lines() {
    assert_eq!(line_diff("a\nb\nc\n", "a\nx\nc\nd\n"), "-b\n+x\n+d\n");
}
