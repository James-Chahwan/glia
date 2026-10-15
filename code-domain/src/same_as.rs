//! Identity keys for SAME_AS classes (DD.2): the rule names, the contract-op
//! key CC.8 and the identity resolver share, per-ecosystem package-name
//! normalisation, the SCIP identity key and the content digests that decide
//! `diverged`.
//!
//! Pure functions over qnames and cells. The identity resolver
//! (`graph/src/resolvers/identity.rs`, DD.5 / DD.6 / DD.7) groups the nodes
//! these keys agree on into one class and writes a SAME_AS star; this
//! module decides nothing about stars, tiers or walks (the tiers are
//! `profile::CODE_TABLES`' identity rules, DD.3).
//!
//! - **Rule names** ([`RULES`]): the EVIDENCE `rule` of a SAME_AS edge is a
//!   rule name, `/diverged` appended when the two members' contents differ
//!   ([`evidence_rule`] / [`split_evidence_rule`]; the `confirms:` rule of the
//!   SCIP stage qualifies with `/` the same way).
//! - **Contract ops** ([`contract_op_key`]): an OpenAPI / AsyncAPI / swaggo /
//!   feature-yaml op DOC_SECTION is keyed from its ORIGIN, `METHOD path` for
//!   an HTTP op and `[action ]channel` for a channel. `contract_breaks` (CC.8)
//!   renders its row keys with the same function, so the two never disagree.
//! - **Packages** ([`dep_identity_key`] / [`project_dep_key`]): a PACKAGE_DEP
//!   `package:<eco>:<name>` and the PROJECT whose manifest names that package
//!   reduce to one `<eco>:<name>` key under the ecosystem's own name rules -
//!   PEP 503 for PyPI, `-` == `_` for crates.io, case-folding for Composer,
//!   verbatim for npm and Go modules. Spike S1 measured every pair.
//! - **SCIP** ([`scip_identity_key`]): a SCIP symbol is `scheme manager
//!   package version descriptor...`; package + descriptor is the identity, the
//!   version is not (two releases of one symbol are one identity whose bodies
//!   may differ).
//! - **Content** ([`code_digest`] / [`content_key`]): xxh64 (seed 0) of a CODE
//!   cell's bytes, or of a MESSAGE_TYPE / DOC_SECTION's SCHEMA_FIELDS payload.
//!   A CODE cell loaded as an unresolved span (CD.7c, `{"code_span":..}`)
//!   carries the xxh64 of exactly those bytes, so the digest is one answer
//!   with or without the sources on disk - what merge == joint build needs.

use glia_core::{Cell, CellPayload, NodeKindId};
use serde_json::Value;

use crate::code_span::{CodeSpan, xxh64};
use crate::{cell_type, node_kind};

// ---- rule names ------------------------------------------------------------

/// Two PACKAGE_DEPs naming one package of one ecosystem ([`dep_identity_key`]).
pub const RULE_PACKAGE_NAME: &str = "package_name";
/// A PROJECT and the PACKAGE_DEPs naming the package its manifest declares
/// ([`project_dep_key`]).
pub const RULE_MANIFEST_NAME: &str = "manifest_name";
/// Two MESSAGE_TYPEs with one qname (flavour and fully-qualified name).
pub const RULE_MESSAGE_FQNAME: &str = "message_fqname";
/// Two copies of one contract op of one project ([`contract_op_key`]).
pub const RULE_CONTRACT_OP: &str = "contract_op";
/// Contract ops anchored to one ROUTE.
pub const RULE_CONTRACT_ROUTE: &str = "contract_route";
/// Definitions two SCIP indexes name with one package + descriptor
/// ([`scip_identity_key`]).
pub const RULE_SCIP_SYMBOL: &str = "scip_symbol";
/// Byte-identical declarations in path-suffix files ([`code_digest`]).
pub const RULE_BODY_HASH: &str = "body_hash";
/// Same-named declarations under one path suffix whose bodies differ.
pub const RULE_PATH_SUFFIX: &str = "path_suffix";

/// Every identity rule name, in rule order.
pub const RULES: [&str; 8] = [
    RULE_PACKAGE_NAME,
    RULE_MANIFEST_NAME,
    RULE_MESSAGE_FQNAME,
    RULE_CONTRACT_OP,
    RULE_CONTRACT_ROUTE,
    RULE_SCIP_SYMBOL,
    RULE_BODY_HASH,
    RULE_PATH_SUFFIX,
];

/// The qualifier of a SAME_AS edge whose two members' contents differ.
pub const DIVERGED: &str = "diverged";

/// The EVIDENCE `rule` of a SAME_AS edge: `rule`, or `rule/diverged`.
pub fn evidence_rule(rule: &str, diverged: bool) -> String {
    if diverged { format!("{rule}/{DIVERGED}") } else { rule.to_string() }
}

/// The inverse of [`evidence_rule`]: `("message_fqname", true)` for
/// `message_fqname/diverged`, `(r, false)` for anything without that suffix.
pub fn split_evidence_rule(r: &str) -> (&str, bool) {
    match r.strip_suffix(DIVERGED).and_then(|b| b.strip_suffix('/')) {
        Some(base) => (base, true),
        None => (r, false),
    }
}

// ---- contract ops ----------------------------------------------------------

/// What a contract op is, read from its ORIGIN.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OpShape {
    /// An HTTP operation: `method` ASCII-upper-cased, `path` verbatim (no
    /// parameter fold; a consumer that matches routes folds it itself), and
    /// the ORIGIN `raw_path` when it has one.
    Http { method: String, path: String, raw_path: Option<String> },
    /// A message channel: the ORIGIN `action` (`publish`, `subscribe`, ..)
    /// when present, and the `channel` with its leading and trailing `/`
    /// trimmed, as the key compares it.
    Channel { action: Option<String>, channel: String },
}

/// A contract op's identity: its shape and the key text CC.8 prints.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContractOpKey {
    pub shape: OpShape,
    /// `GET /users/{id}`, `publish orders` or `orders`.
    pub display: String,
}

/// The key of the contract op an ORIGIN describes. The ORIGIN must say
/// `provenance: contract`. When `method` and `path` are both strings it is an
/// HTTP op, displayed `METHOD path`; otherwise `action` + `channel` display as
/// `action channel` and `channel` alone as `channel` (the channel trimmed of
/// `/` both ends); an ORIGIN with neither is `None`.
pub fn contract_op_key(origin: &Value) -> Option<ContractOpKey> {
    if origin.get("provenance").and_then(Value::as_str) != Some("contract") {
        return None;
    }
    let field = |k: &str| origin.get(k).and_then(Value::as_str);
    if let (Some(method), Some(path)) = (field("method"), field("path")) {
        let method = method.to_ascii_uppercase();
        let display = format!("{method} {path}");
        let shape = OpShape::Http {
            method,
            path: path.to_string(),
            raw_path: field("raw_path").map(str::to_string),
        };
        return Some(ContractOpKey { shape, display });
    }
    let channel = field("channel")?.trim_matches('/');
    let action = field("action");
    let display = match action {
        Some(a) => format!("{a} {channel}"),
        None => channel.to_string(),
    };
    let shape = OpShape::Channel { action: action.map(str::to_string), channel: channel.to_string() };
    Some(ContractOpKey { shape, display })
}

/// [`contract_op_key`] of an ORIGIN payload's JSON text. A payload that does
/// not hold the string `"contract"` is rejected before it is parsed.
pub fn contract_op_key_json(origin_json: &str) -> Option<ContractOpKey> {
    if !origin_json.contains("\"contract\"") {
        return None;
    }
    let v: Value = serde_json::from_str(origin_json).ok()?;
    contract_op_key(&v)
}

/// The key of the first ORIGIN cell of `cells` that describes a contract op.
pub fn contract_op_key_cells(cells: &[Cell]) -> Option<ContractOpKey> {
    cells
        .iter()
        .filter(|c| c.kind == cell_type::ORIGIN)
        .filter_map(|c| payload_str(&c.payload))
        .find_map(contract_op_key_json)
}

// ---- packages --------------------------------------------------------------

/// The identity key of a PACKAGE_DEP qname `package:<eco>:<name>`:
/// `<eco>:<name>` with the name normalised as its ecosystem defines one
/// package name - `pypi` PEP 503 (ASCII-lowercase, every run of `-` `_` `.`
/// one `-`), `cargo` `-` read as `_`, `composer` ASCII-lowercase; `npm`,
/// `gomod` and every other ecosystem verbatim. `None` for another qname shape
/// or an empty ecosystem or name.
pub fn dep_identity_key(dep_qname: &str) -> Option<String> {
    let (eco, name) = dep_qname.strip_prefix("package:")?.split_once(':')?;
    dep_key(eco, name)
}

/// The [`dep_identity_key`] a PROJECT stands for: its manifest ecosystem
/// mapped to the PACKAGE_DEP ecosystem (`npm` -> `npm`, `go` -> `gomod`,
/// `cargo` -> `cargo`, `python` -> `pypi`, `php` -> `composer`) and its label
/// normalised the same way. `None` for every other PROJECT ecosystem (maven,
/// gradle, dart, elixir, dotnet and the rest extract no PACKAGE_DEP; a
/// Gemfile names no package; a declared root has no manifest) and for an
/// empty label.
pub fn project_dep_key(ecosystem: &str, label: &str) -> Option<String> {
    let eco = match ecosystem {
        "npm" => "npm",
        "go" => "gomod",
        "cargo" => "cargo",
        "python" => "pypi",
        "php" => "composer",
        _ => return None,
    };
    dep_key(eco, label)
}

fn dep_key(eco: &str, name: &str) -> Option<String> {
    if eco.is_empty() || name.is_empty() {
        return None;
    }
    let name = match eco {
        "pypi" => pep503(name),
        "cargo" => name.replace('-', "_"),
        "composer" => name.to_ascii_lowercase(),
        _ => name.to_string(),
    };
    Some(format!("{eco}:{name}"))
}

/// PEP 503 normalisation: `re.sub(r"[-_.]+", "-", name).lower()`.
fn pep503(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut in_run = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !in_run {
                out.push('-');
            }
            in_run = true;
        } else {
            out.push(c.to_ascii_lowercase());
            in_run = false;
        }
    }
    out
}

// ---- SCIP symbols ----------------------------------------------------------

/// A SCIP symbol read as an identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScipIdentity {
    /// `scheme manager package descriptor`: the symbol with its version field
    /// cut out, each field in SCIP's own encoding (a space doubled, an empty
    /// field `.`), so two keys are equal exactly when the four parts are.
    pub key: String,
    /// The version field, decoded (`.` reads as empty).
    pub version: String,
    /// The scheme, decoded.
    pub scheme: String,
    /// The package manager, decoded (`.` reads as empty).
    pub manager: String,
    /// The package name, decoded (`.` reads as empty).
    pub package: String,
    /// The descriptor suffix, verbatim.
    pub descriptor: String,
}

/// The identity of a global SCIP symbol (`scheme manager package version
/// descriptor...`). The first four fields end at a single space, a doubled
/// space inside one is a literal space, and `.` in manager / package /
/// version is an empty value; the rest is the descriptor, verbatim. `None`
/// for a `local <id>` symbol, an empty scheme, fewer than five fields or an
/// empty descriptor.
pub fn scip_identity_key(symbol: &str) -> Option<ScipIdentity> {
    if symbol.starts_with("local ") {
        return None;
    }
    let mut fields: Vec<String> = Vec::with_capacity(4);
    let mut cur = String::new();
    let mut rest_at = None;
    let mut chars = symbol.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != ' ' {
            cur.push(c);
            continue;
        }
        if chars.peek().is_some_and(|&(_, n)| n == ' ') {
            chars.next();
            cur.push(' ');
            continue;
        }
        fields.push(std::mem::take(&mut cur));
        if fields.len() == 4 {
            // `' '` is one byte, so `i + 1` is a char boundary.
            rest_at = Some(i + 1);
            break;
        }
    }
    let descriptor = symbol.get(rest_at?..)?;
    if descriptor.is_empty() {
        return None;
    }
    let mut it = fields.into_iter();
    let scheme = it.next()?;
    if scheme.is_empty() {
        return None;
    }
    let placeholder = |f: String| if f == "." { String::new() } else { f };
    let manager = placeholder(it.next()?);
    let package = placeholder(it.next()?);
    let version = placeholder(it.next()?);
    let key = format!(
        "{} {} {} {descriptor}",
        scip_field(&scheme),
        scip_field(&manager),
        scip_field(&package)
    );
    Some(ScipIdentity {
        key,
        version,
        scheme,
        manager,
        package,
        descriptor: descriptor.to_string(),
    })
}

/// A decoded SCIP field back in SCIP's encoding: spaces doubled, empty `.`.
fn scip_field(f: &str) -> String {
    if f.is_empty() { ".".to_string() } else { f.replace(' ', "  ") }
}

// ---- content digests -------------------------------------------------------

/// The shortest CODE, in bytes, a digest is taken of: a one-line body matches
/// too much to be evidence (graph `identity`'s MIN_BODY_CHARS reasoning).
pub const MIN_DIGEST_BYTES: usize = 40;

/// xxh64 (seed 0) of the first CODE cell's bytes: its text, or, for a CODE
/// cell loaded as an unresolved span (`{"code_span":..}`, CD.7c), the span's
/// own xxh64, which hashes the same bytes. `None` without a CODE cell, for a
/// CODE cell shorter than [`MIN_DIGEST_BYTES`] and for a bytes payload.
pub fn code_digest(cells: &[Cell]) -> Option<u64> {
    let code = cells.iter().find(|c| c.kind == cell_type::CODE)?;
    if let Some(span) = CodeSpan::from_payload(&code.payload) {
        return (span.len() >= MIN_DIGEST_BYTES as u64).then_some(span.xxh64);
    }
    let text = payload_str(&code.payload)?;
    (text.len() >= MIN_DIGEST_BYTES).then(|| xxh64(text.as_bytes()))
}

/// What decides `diverged` for a node of `kind`: a MESSAGE_TYPE or
/// DOC_SECTION by xxh64 of its first SCHEMA_FIELDS payload (`None` without
/// one), every other kind by [`code_digest`].
pub fn content_key(kind: NodeKindId, cells: &[Cell]) -> Option<u64> {
    if kind == node_kind::MESSAGE_TYPE || kind == node_kind::DOC_SECTION {
        return cells
            .iter()
            .filter(|c| c.kind == cell_type::SCHEMA_FIELDS)
            .find_map(|c| payload_str(&c.payload))
            .map(|p| xxh64(p.as_bytes()));
    }
    code_digest(cells)
}

fn payload_str(p: &CellPayload) -> Option<&str> {
    match p {
        CellPayload::Text(s) | CellPayload::Json(s) => Some(s),
        CellPayload::Bytes(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cell(kind: glia_core::CellTypeId, payload: CellPayload) -> Cell {
        Cell { kind, payload }
    }

    fn display(v: Value) -> Option<String> {
        contract_op_key(&v).map(|k| k.display)
    }

    #[test]
    fn op_key_matches_cc8() {
        let k = contract_op_key(&json!({
            "provenance": "contract", "source": "openapi", "method": "get", "path": "/users/{id}",
        }))
        .expect("http op");
        assert_eq!(k.display, "GET /users/{id}");
        assert_eq!(
            k.shape,
            OpShape::Http { method: "GET".into(), path: "/users/{id}".into(), raw_path: None }
        );
        for source in ["swaggo", "feature_yaml"] {
            let k = contract_op_key(&json!({
                "provenance": "contract", "source": source, "method": "Post",
                "path": "/orders", "raw_path": "/orders/",
            }))
            .expect("http op");
            assert_eq!(k.display, "POST /orders", "{source}");
            assert_eq!(
                k.shape,
                OpShape::Http {
                    method: "POST".into(),
                    path: "/orders".into(),
                    raw_path: Some("/orders/".into()),
                }
            );
        }
        let k = contract_op_key(&json!({
            "provenance": "contract", "source": "asyncapi", "action": "publish", "channel": "/orders/",
        }))
        .expect("channel op");
        assert_eq!(k.display, "publish orders");
        assert_eq!(
            k.shape,
            OpShape::Channel { action: Some("publish".into()), channel: "orders".into() }
        );
        assert_eq!(display(json!({"provenance": "contract", "channel": "/x/"})).as_deref(), Some("x"));
        // key_of's fall-through: a method without a string path is not HTTP.
        assert_eq!(display(json!({"provenance": "contract", "method": "get"})), None);
        assert_eq!(
            display(json!({"provenance": "contract", "method": "get", "channel": "c"})).as_deref(),
            Some("c")
        );
        assert_eq!(
            display(json!({"provenance": "contract", "method": "get", "path": 3, "action": "a", "channel": "c"}))
                .as_deref(),
            Some("a c")
        );
        // An action without a channel keys nothing (key_of keeps its qname).
        assert_eq!(display(json!({"provenance": "contract", "action": "publish"})), None);
        assert_eq!(
            display(json!({"provenance": "documentation", "method": "get", "path": "/u"})),
            None
        );
        assert_eq!(display(json!({"method": "get", "path": "/u"})), None);
        assert_eq!(display(json!("contract")), None);
    }

    #[test]
    fn op_key_json_and_cells() {
        let j = r#"{"provenance":"contract","source":"openapi","method":"delete","path":"/a"}"#;
        assert_eq!(contract_op_key_json(j).map(|k| k.display).as_deref(), Some("DELETE /a"));
        // No `"contract"` string: rejected before parsing (valid or not).
        assert_eq!(contract_op_key_json(r#"{"provenance":"code","method":"get","path":"/a"}"#), None);
        assert_eq!(contract_op_key_json("not json at all"), None);
        assert_eq!(contract_op_key_json(r#"{"provenance":"contract", broken"#), None);
        let cells = vec![
            cell(cell_type::CODE, CellPayload::Text("x".into())),
            cell(cell_type::ORIGIN, CellPayload::Json(r#"{"provenance":"code"}"#.into())),
            cell(cell_type::ORIGIN, CellPayload::Json(j.into())),
        ];
        assert_eq!(contract_op_key_cells(&cells).map(|k| k.display).as_deref(), Some("DELETE /a"));
        assert_eq!(contract_op_key_cells(&cells[..2]), None);
        // A contract ORIGIN on a non-ORIGIN cell is not read.
        assert_eq!(contract_op_key_cells(&[cell(cell_type::CODE, CellPayload::Json(j.into()))]), None);
    }

    #[test]
    fn s1_ecosystem_table() {
        let pairs = [
            (("npm", "@org/lib"), "package:npm:@org/lib"),
            (("go", "github.com/org/lib"), "package:gomod:github.com/org/lib"),
            (("cargo", "my-lib"), "package:cargo:my_lib"),
            (("python", "My_Lib"), "package:pypi:my-lib"),
            (("php", "org/lib"), "package:composer:org/lib"),
        ];
        for ((eco, label), dep) in pairs {
            let p = project_dep_key(eco, label);
            assert!(p.is_some(), "{eco} {label}");
            assert_eq!(p, dep_identity_key(dep), "{eco} {label} vs {dep}");
        }
        assert_eq!(project_dep_key("python", "My_Lib").as_deref(), Some("pypi:my-lib"));
        assert_eq!(project_dep_key("cargo", "my-lib").as_deref(), Some("cargo:my_lib"));
        for eco in ["maven", "ruby", "dart", "declared", "gradle", "elixir", "dotnet", "swift"] {
            assert_eq!(project_dep_key(eco, "lib"), None, "{eco}");
        }
        assert_eq!(project_dep_key("npm", ""), None);
        assert_ne!(dep_identity_key("package:npm:requests"), dep_identity_key("package:pypi:requests"));
        // npm stays case-sensitive; Composer folds case; PEP 503 folds runs.
        assert_ne!(dep_identity_key("package:npm:React"), dep_identity_key("package:npm:react"));
        assert_eq!(dep_identity_key("package:composer:Org/Lib").as_deref(), Some("composer:org/lib"));
        assert_eq!(dep_identity_key("package:pypi:Zope.._-Interface").as_deref(), Some("pypi:zope-interface"));
        assert_eq!(dep_identity_key("package:rubygems:rails").as_deref(), Some("rubygems:rails"));
        assert_eq!(dep_identity_key("package:npm:"), None);
        assert_eq!(dep_identity_key("package::x"), None);
        assert_eq!(dep_identity_key("module:npm:x"), None);
        assert_eq!(dep_identity_key("package:npm"), None);
    }

    #[test]
    fn scip_keys() {
        let s = "scip-python python svc 0.1 `svc.repos`/UserRepo#save().";
        let id = scip_identity_key(s).expect("global symbol");
        assert_eq!(id.key, "scip-python python svc `svc.repos`/UserRepo#save().");
        assert_eq!(id.version, "0.1");
        assert_eq!(id.package, "svc");
        assert_eq!(id.descriptor, "`svc.repos`/UserRepo#save().");
        let later = scip_identity_key("scip-python python svc 0.2 `svc.repos`/UserRepo#save().")
            .expect("global symbol");
        assert_eq!(later.key, id.key);
        assert_ne!(later.version, id.version);
        assert_ne!(
            scip_identity_key("scip-python python svc 0.1 `svc.repos`/UserRepo#load().").map(|i| i.key),
            Some(id.key.clone())
        );
        assert_eq!(scip_identity_key("local 4"), None);
        let esc = scip_identity_key("scip-x npm my  pkg 1.0 a/b().").expect("escaped space");
        assert_eq!(esc.package, "my pkg");
        assert_eq!(esc.version, "1.0");
        assert_eq!(esc.descriptor, "a/b().");
        assert_eq!(esc.key, "scip-x npm my  pkg a/b().");
        // Scheme `a b` + manager `c` and scheme `a` + manager `b c` are two
        // keys: the key keeps SCIP's escaping, so it is injective.
        assert_ne!(
            scip_identity_key("a  b c p v d.").map(|i| i.key),
            scip_identity_key("a b  c p v d.").map(|i| i.key)
        );
        let dots = scip_identity_key("rust-analyzer . . . crate/f().").expect("placeholders");
        assert_eq!((dots.manager.as_str(), dots.package.as_str(), dots.version.as_str()), ("", "", ""));
        assert_eq!(dots.key, "rust-analyzer . . crate/f().");
        // The descriptor is the verbatim rest, spaces included.
        let sp = scip_identity_key("s m p v `a b`/c.").expect("descriptor with a space");
        assert_eq!(sp.descriptor, "`a b`/c.");
        // Multibyte text walks by char.
        let u = scip_identity_key("s m pä  ké v d/é().").expect("utf-8");
        assert_eq!(u.package, "pä ké");
        assert_eq!(u.descriptor, "d/é().");
        assert_eq!(scip_identity_key("scip-python python svc 0.1"), None);
        assert_eq!(scip_identity_key("scip-python python svc 0.1 "), None);
        assert_eq!(scip_identity_key(" python svc 0.1 d."), None);
        assert_eq!(scip_identity_key(""), None);
    }

    #[test]
    fn digests() {
        let t39 = "a".repeat(39);
        let t40 = "fn f() { let x = 1; return x + 1; }   ..".to_string();
        assert_eq!(t40.len(), MIN_DIGEST_BYTES);
        let code = |s: &str| vec![cell(cell_type::CODE, CellPayload::Text(s.to_string()))];
        assert_eq!(code_digest(&code(&t39)), None);
        assert_eq!(code_digest(&code(&t40)), Some(xxh64(t40.as_bytes())));
        let span = CodeSpan::of("src/a.rs", 120, t40.as_bytes()).to_payload();
        assert_eq!(code_digest(&[cell(cell_type::CODE, span)]), code_digest(&code(&t40)));
        let short = CodeSpan::of("src/a.rs", 0, t39.as_bytes()).to_payload();
        assert_eq!(code_digest(&[cell(cell_type::CODE, short)]), None);
        assert_eq!(code_digest(&[cell(cell_type::CODE, CellPayload::Bytes(vec![0x02; 64]))]), None);
        assert_eq!(code_digest(&[cell(cell_type::ORIGIN, CellPayload::Text(t40.clone()))]), None);
        assert_eq!(code_digest(&[]), None);
        // The first CODE cell decides, even when a later one is long enough.
        let two = vec![
            cell(cell_type::CODE, CellPayload::Text(t39.clone())),
            cell(cell_type::CODE, CellPayload::Text(t40.clone())),
        ];
        assert_eq!(code_digest(&two), None);

        let p = r#"{"format":"proto","fields":[{"name":"id","type":"string","number":1}]}"#;
        let message = vec![
            cell(cell_type::CODE, CellPayload::Text(t40.clone())),
            cell(cell_type::SCHEMA_FIELDS, CellPayload::Json(p.into())),
        ];
        assert_eq!(content_key(node_kind::MESSAGE_TYPE, &message), Some(xxh64(p.as_bytes())));
        assert_eq!(content_key(node_kind::DOC_SECTION, &message), Some(xxh64(p.as_bytes())));
        assert_eq!(content_key(node_kind::MESSAGE_TYPE, &code(&t40)), None);
        assert_eq!(content_key(node_kind::FUNCTION, &message), Some(xxh64(t40.as_bytes())));
        assert_eq!(content_key(node_kind::FUNCTION, &code(&t39)), None);
    }

    #[test]
    fn evidence_rule_round_trip() {
        for rule in RULES {
            for diverged in [false, true] {
                let r = evidence_rule(rule, diverged);
                assert_eq!(split_evidence_rule(&r), (rule, diverged), "{r}");
            }
        }
        assert_eq!(evidence_rule(RULE_MESSAGE_FQNAME, true), "message_fqname/diverged");
        assert_eq!(evidence_rule(RULE_MESSAGE_FQNAME, false), "message_fqname");
        assert_eq!(split_evidence_rule("diverged"), ("diverged", false));
        assert_eq!(split_evidence_rule("xdiverged"), ("xdiverged", false));
        let mut sorted = RULES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), RULES.len());
    }
}
