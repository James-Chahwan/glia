//! Secrets-manager references and feature-flag checks as `CONFIG_KEY`
//! flavours (A13.8).
//!
//! No new kind, edge or cell id: a secret is `config:secret:<provider>/<ref>`,
//! a flag is `config:flag:<key>`, both reached by `READS_CONFIG` from the
//! module that names them and emitted through config.rs's one `build_nodes`
//! path. The ENV cell records the provider and `"redacted":true` — a read
//! site states no value, and a secret's value must never reach the graph.
//!
//! Both scanners are language-blind string scans, the established shape for
//! this crate (data_sources.rs documents the trade-off): the same SDK idiom
//! crosses languages, and a tree-sitter pass per language would cost more
//! than it buys for a literal first argument.
//!
//! PRECISION. `.read(`, `.isEnabled(`, `.variation(` and `.getValue(` are
//! generic method names, and a key-shape check alone does not stop
//! `feature.isEnabled("x-y")` in unrelated code. So every needle row carries
//! PROVIDER TOKENS, and a row fires only in a file that mentions one of them
//! (case-insensitively) — in practice, the file that imports the SDK. The
//! captured argument must then be exactly one string literal (a concatenation,
//! f-string or variable is skipped), and the flavor validator in config.rs
//! rejects interpolated secret refs and short flag keys.
//!
//! The yaml side — k8s `secretKeyRef:` reads, `kind: Secret` and Flipt
//! `flags:` definitions — lives in config.rs's `extract_yaml_env_defs`, which
//! already walks every yaml file once.

use glia_core::{NodeId, RepoId};

use crate::config::{ConfigDef, ConfigNodes, Side, build_nodes};

/// How the key is read off a matched call.
#[derive(Clone, Copy)]
enum Pick {
    /// The n-th positional argument, which must be exactly one string literal.
    Positional(usize),
    /// A named argument / object-literal key / struct field inside the call
    /// (`SecretId="x"`, `{ SecretId: 'x' }`, `SecretId: aws.String("x")`).
    Named(&'static [&'static str]),
    /// [`Pick::Named`], else the first positional literal.
    NamedOrFirst(&'static [&'static str]),
    /// Split's server SDKs take `(userKey, flagName)` and the browser SDK
    /// takes `(flagName)`: the second argument if it is a literal, else the
    /// first.
    SecondElseFirst,
    /// A mount-scoped Vault client: `KVv2("secret").Get(ctx, "db")` — the
    /// mount is this call's first literal, the path is argument `n` of the
    /// chained `method` call. Yields `mount/path`.
    MountThen(&'static str, usize),
}

struct Row {
    /// Text immediately before the call's opening `(` / `{` (whitespace and a
    /// C# `()` may sit between them).
    needle: &'static str,
    provider: &'static str,
    /// Lowercase tokens; the row fires only if the file contains one.
    gate: &'static [&'static str],
    pick: Pick,
    /// Named arguments holding a mount prefix (`mount_point="secret"`).
    mount: &'static [&'static str],
    norm: fn(&str) -> Option<String>,
}

const VAULT: &[&str] = &["hvac", "vault"];
const AWS_SM: &[&str] = &["secretsmanager", "secrets-manager"];
const GCP_SM: &[&str] = &["secretmanager"];
const AZURE_KV: &[&str] = &["keyvault"];

const fn secret(
    needle: &'static str,
    provider: &'static str,
    gate: &'static [&'static str],
    pick: Pick,
    mount: &'static [&'static str],
    norm: fn(&str) -> Option<String>,
) -> Row {
    Row { needle, provider, gate, pick, mount, norm }
}

const SECRET_ROWS: &[Row] = &[
    // Vault — hvac (py), node-vault (js), Spring VaultTemplate (java),
    // `Vault.logical.read` (ruby) all take the full path as the first
    // argument. `.read(` is generic, so the path must contain a `/`.
    secret(".read(", "vault", VAULT, Pick::Positional(0), &[], vault_path),
    secret(".Logical().Read(", "vault", VAULT, Pick::Positional(0), &[], vault_path),
    secret(".Logical().ReadWithContext(", "vault", VAULT, Pick::Positional(1), &[], vault_path),
    // hvac KV engines: `read_secret_version(path="api", mount_point="secret")`.
    secret(".read_secret_version(", "vault", VAULT, Pick::NamedOrFirst(&["path"]), &["mount_point"], plain),
    secret(".read_secret(", "vault", VAULT, Pick::NamedOrFirst(&["path"]), &["mount_point"], plain),
    // VaultSharp: `ReadSecretAsync(path: "db", mountPoint: "secret")`.
    secret(".ReadSecretAsync(", "vault", VAULT, Pick::NamedOrFirst(&["path"]), &["mountPoint"], plain),
    // Go `client.KVv2("secret").Get(ctx, "db")`; Ruby `Vault.kv("secret").read("db")`.
    secret(".KVv2(", "vault", VAULT, Pick::MountThen(".Get(", 1), &[], plain),
    secret(".KVv1(", "vault", VAULT, Pick::MountThen(".Get(", 1), &[], plain),
    secret(".kv(", "vault", VAULT, Pick::MountThen(".read(", 0), &[], plain),
    // AWS Secrets Manager — boto3 kwarg, JS v2 / v3 object literal, Go input
    // struct (`aws.String` wrapper), C# initializer, Java v1 / v2 builders.
    secret(".get_secret_value(", "aws_sm", AWS_SM, Pick::Named(&["SecretId"]), &[], aws_ref),
    secret(".getSecretValue(", "aws_sm", AWS_SM, Pick::Named(&["SecretId"]), &[], aws_ref),
    secret("GetSecretValueCommand(", "aws_sm", AWS_SM, Pick::Named(&["SecretId"]), &[], aws_ref),
    secret("GetSecretValueInput", "aws_sm", AWS_SM, Pick::Named(&["SecretId"]), &[], aws_ref),
    secret("GetSecretValueRequest", "aws_sm", AWS_SM, Pick::Named(&["SecretId"]), &[], aws_ref),
    secret(".secretId(", "aws_sm", AWS_SM, Pick::Positional(0), &[], aws_ref),
    secret(".withSecretId(", "aws_sm", AWS_SM, Pick::Positional(0), &[], aws_ref),
    // GCP Secret Manager: a `projects/<p>/secrets/<s>[/versions/<v>]` name.
    secret(".access_secret_version(", "gcp_sm", GCP_SM, Pick::NamedOrFirst(&["name"]), &[], gcp_ref),
    secret(".accessSecretVersion(", "gcp_sm", GCP_SM, Pick::NamedOrFirst(&["name"]), &[], gcp_ref),
    secret("AccessSecretVersionRequest", "gcp_sm", GCP_SM, Pick::Named(&["Name"]), &[], gcp_ref),
    // Azure Key Vault `SecretClient`: the secret's name.
    secret(".get_secret(", "azure_kv", AZURE_KV, Pick::NamedOrFirst(&["name"]), &[], plain),
    secret(".getSecret(", "azure_kv", AZURE_KV, Pick::Positional(0), &[], plain),
    secret(".GetSecret(", "azure_kv", AZURE_KV, Pick::NamedOrFirst(&["name"]), &[], plain),
    secret(".GetSecretAsync(", "azure_kv", AZURE_KV, Pick::NamedOrFirst(&["name"]), &[], plain),
];

const LAUNCHDARKLY: &[&str] = &["launchdarkly", "ldclient"];
const OPENFEATURE: &[&str] = &["openfeature"];
const UNLEASH: &[&str] = &["unleash"];
const FLAGSMITH: &[&str] = &["flagsmith"];
const SPLIT: &[&str] = &["splitio", "io.split"];

const fn flag(needle: &'static str, provider: &'static str, gate: &'static [&'static str], pick: Pick) -> Row {
    Row { needle, provider, gate, pick, mount: &[], norm: plain }
}

const FLAG_ROWS: &[Row] = &[
    // LaunchDarkly: `variation("k", ctx, default)` in every server SDK;
    // `Variation(` covers Bool / String / Int / Float64 / JSON / bool / json
    // / jsonValue ... and `VariationDetail(` their detail twins.
    flag(".variation(", "launchdarkly", LAUNCHDARKLY, Pick::Positional(0)),
    flag("Variation(", "launchdarkly", LAUNCHDARKLY, Pick::Positional(0)),
    flag(".variation_detail(", "launchdarkly", LAUNCHDARKLY, Pick::Positional(0)),
    flag(".variationDetail(", "launchdarkly", LAUNCHDARKLY, Pick::Positional(0)),
    flag("VariationDetail(", "launchdarkly", LAUNCHDARKLY, Pick::Positional(0)),
    // OpenFeature: key first everywhere but Go, where it follows the ctx.
    flag(".getBooleanValue(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".getStringValue(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".getNumberValue(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".getIntegerValue(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".getDoubleValue(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".getObjectValue(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".getBooleanDetails(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".get_boolean_value(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".get_string_value(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".get_integer_value(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".get_float_value(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".get_object_value(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".GetBooleanValue(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".GetBooleanValueAsync(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".GetStringValueAsync(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".GetIntegerValueAsync(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".GetDoubleValueAsync(", "openfeature", OPENFEATURE, Pick::Positional(0)),
    flag(".BooleanValue(", "openfeature", OPENFEATURE, Pick::Positional(1)),
    flag(".StringValue(", "openfeature", OPENFEATURE, Pick::Positional(1)),
    flag(".IntValue(", "openfeature", OPENFEATURE, Pick::Positional(1)),
    flag(".FloatValue(", "openfeature", OPENFEATURE, Pick::Positional(1)),
    flag(".ObjectValue(", "openfeature", OPENFEATURE, Pick::Positional(1)),
    // Unleash: JS / Java camelCase, Python snake_case, Go / C# PascalCase,
    // Ruby predicate.
    flag(".isEnabled(", "unleash", UNLEASH, Pick::Positional(0)),
    flag(".is_enabled(", "unleash", UNLEASH, Pick::Positional(0)),
    flag(".IsEnabled(", "unleash", UNLEASH, Pick::Positional(0)),
    flag(".is_enabled?(", "unleash", UNLEASH, Pick::Positional(0)),
    flag(".getVariant(", "unleash", UNLEASH, Pick::Positional(0)),
    flag(".get_variant(", "unleash", UNLEASH, Pick::Positional(0)),
    flag(".GetVariant(", "unleash", UNLEASH, Pick::Positional(0)),
    // Flagsmith.
    flag(".hasFeature(", "flagsmith", FLAGSMITH, Pick::Positional(0)),
    flag(".has_feature(", "flagsmith", FLAGSMITH, Pick::Positional(0)),
    flag(".is_feature_enabled(", "flagsmith", FLAGSMITH, Pick::Positional(0)),
    flag(".isFeatureEnabled(", "flagsmith", FLAGSMITH, Pick::Positional(0)),
    flag(".IsFeatureEnabled(", "flagsmith", FLAGSMITH, Pick::Positional(0)),
    flag(".getValue(", "flagsmith", FLAGSMITH, Pick::Positional(0)),
    flag(".get_feature_value(", "flagsmith", FLAGSMITH, Pick::Positional(0)),
    flag(".getFeatureValue(", "flagsmith", FLAGSMITH, Pick::Positional(0)),
    flag(".GetFeatureValue(", "flagsmith", FLAGSMITH, Pick::Positional(0)),
    // Split.
    flag(".getTreatment(", "split", SPLIT, Pick::SecondElseFirst),
    flag(".get_treatment(", "split", SPLIT, Pick::SecondElseFirst),
    flag(".getTreatmentWithConfig(", "split", SPLIT, Pick::SecondElseFirst),
    flag(".get_treatment_with_config(", "split", SPLIT, Pick::SecondElseFirst),
    flag(".Treatment(", "split", SPLIT, Pick::SecondElseFirst),
];

/// Secrets-manager references read in a code file:
/// `config:secret:<provider>/<ref>`, `READS_CONFIG` from `module_id`.
pub fn extract_secret_refs(source: &str, module_id: NodeId, repo: RepoId) -> ConfigNodes {
    let defs = scan(source, SECRET_ROWS)
        .into_iter()
        .map(|(name, provider)| ConfigDef::secret(name, provider))
        .collect();
    build_nodes(defs, Side::Read, module_id, repo)
}

/// Feature-flag checks in a code file: `config:flag:<key>`, `READS_CONFIG`
/// from `module_id`, the SDK named on the ENV cell's `source`.
pub fn extract_feature_flags(source: &str, module_id: NodeId, repo: RepoId) -> ConfigNodes {
    let defs = scan(source, FLAG_ROWS)
        .into_iter()
        .map(|(name, provider)| ConfigDef::flag(name, provider))
        .collect();
    build_nodes(defs, Side::Read, module_id, repo)
}

/// The per-file `[secrets]` fired-on marker over what one file produced, or
/// `None` when it captured no secret or flag. `src` is the discriminator —
/// the language tag for a code file, `yaml` for a manifest.
///
/// `refs` counts secret keys on either side, `providers` their distinct
/// providers, `flags` / `flag_defs` flag keys read / declared.
pub fn marker(outs: &[&ConfigNodes], src: &str) -> Option<String> {
    use glia_code_domain::edge_category;
    use std::collections::BTreeSet;
    let (mut refs, mut providers) = (BTreeSet::new(), BTreeSet::new());
    let (mut flags, mut flag_defs) = (BTreeSet::new(), BTreeSet::new());
    for out in outs {
        for e in &out.edges {
            let Some(q) = out.nav.qname_by_id.get(&e.to) else {
                continue;
            };
            if let Some(rest) = q.strip_prefix("config:secret:") {
                refs.insert(q.as_str());
                providers.insert(rest.split('/').next().unwrap_or(rest));
            } else if q.starts_with("config:flag:") {
                if e.category == edge_category::DEFINES_CONFIG {
                    flag_defs.insert(q.as_str());
                } else {
                    flags.insert(q.as_str());
                }
            }
        }
    }
    if refs.is_empty() && flags.is_empty() && flag_defs.is_empty() {
        return None;
    }
    Some(format!(
        "[secrets] refs={} providers={} flags={} flag_defs={} src={src}",
        refs.len(),
        providers.len(),
        flags.len(),
        flag_defs.len()
    ))
}

// ----------------------------------------------------------------------------
// Scanner
// ----------------------------------------------------------------------------

/// Every `(key, provider)` the rows capture in `source`, in source order per
/// row. A row whose provider tokens are absent from the file never runs.
fn scan(source: &str, rows: &[Row]) -> Vec<(String, &'static str)> {
    let mut lower: Option<String> = None;
    let mut out = Vec::new();
    for row in rows {
        if !source.contains(row.needle) {
            continue;
        }
        let lower = lower.get_or_insert_with(|| source.to_ascii_lowercase());
        if !row.gate.iter().any(|tok| lower.contains(tok)) {
            continue;
        }
        let mut from = 0;
        while let Some(rel) = source[from..].find(row.needle) {
            let after = from + rel + row.needle.len();
            // A needle ending in `(` ends ON the opener; any other needle
            // (`GetSecretValueInput`) is followed by one.
            let call = if row.needle.ends_with('(') {
                Some(&source[after - 1..])
            } else {
                call_opener(&source[after..]).map(|o| &source[after + o..])
            };
            if let Some(key) = call.and_then(|c| capture(c, row)) {
                out.push((key, row.provider));
            }
            from = after;
        }
    }
    out
}

/// The key one matched call names, or `None` when it is not a literal.
/// `call` starts at the call's opening `(` / `{`.
fn capture(call: &str, row: &Row) -> Option<String> {
    let (args, close) = split_args(call)?;
    let raw = match row.pick {
        Pick::Positional(n) => args.get(n).and_then(|a| string_literal(a))?.to_string(),
        Pick::Named(names) => named_literal(&call[1..close - 1], names)?,
        Pick::NamedOrFirst(names) => named_literal(&call[1..close - 1], names)
            .or_else(|| args.first().and_then(|a| string_literal(a)).map(str::to_string))?,
        Pick::SecondElseFirst => args
            .get(1)
            .and_then(|a| string_literal(a))
            .or_else(|| args.first().and_then(|a| string_literal(a)))?
            .to_string(),
        Pick::MountThen(method, n) => {
            let mount = args.first().and_then(|a| string_literal(a))?;
            let rest = call[close..].trim_start().strip_prefix(method)?;
            // `method` ends at its `(`: step back onto it for `split_args`.
            let (chained, _) = split_args(&call[call.len() - rest.len() - 1..])?;
            let path = chained.get(n).and_then(|a| string_literal(a))?;
            join_mount(mount, path)
        }
    };
    let key = match named_literal_opt(row.mount, &call[1..close - 1]) {
        Some(mount) => join_mount(&mount, &raw),
        None => raw,
    };
    (row.norm)(&key)
}

fn named_literal_opt(names: &[&str], window: &str) -> Option<String> {
    if names.is_empty() {
        None
    } else {
        named_literal(window, names)
    }
}

fn join_mount(mount: &str, path: &str) -> String {
    format!("{}/{}", mount.trim_matches('/'), path.trim_start_matches('/'))
}

/// Byte offset of the opening `(` / `{` after a needle that stops at a type
/// name (`GetSecretValueRequest`): whitespace, an optional C# `()`, then the
/// opener. Anything else (`.builder()`, `;` of an import) is not a call.
fn call_opener(after: &str) -> Option<usize> {
    let b = after.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    // A C# `new GetSecretValueRequest() { SecretId = "x" }`.
    if after[i..].starts_with("()") {
        let mut j = i + 2;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if b.get(j) == Some(&b'{') {
            return Some(j);
        }
    }
    matches!(b.get(i), Some(b'(' | b'{')).then_some(i)
}

/// The argument list opened at `s[0]` (`(` / `{` / `[`), split at depth-0
/// commas, plus the byte offset just past its closer. String literals
/// (`'` `"` `` ` ``, backslash escapes) are skipped whole, so a comma or
/// bracket inside one never splits. Gives up past `LIMIT` bytes.
fn split_args(s: &str) -> Option<(Vec<&str>, usize)> {
    const LIMIT: usize = 2048;
    let b = s.as_bytes();
    if !matches!(b.first(), Some(b'(' | b'{' | b'[')) {
        return None;
    }
    let mut depth = 0usize;
    let mut args = Vec::new();
    let mut start = 1;
    let mut i = 0;
    while i < b.len() && i < LIMIT {
        match b[i] {
            q @ (b'"' | b'\'' | b'`') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let last = s.get(start..i)?;
                    if !last.trim().is_empty() || !args.is_empty() {
                        args.push(last);
                    }
                    return Some((args, i + 1));
                }
            }
            b',' if depth == 1 => {
                args.push(s.get(start..i)?);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The inner text of `arg` iff it is exactly ONE string literal — `"x"`,
/// `'x'` or `` `x` ``. A concatenation, a format call, an f-string or a
/// variable is not a key the graph can name.
fn string_literal(arg: &str) -> Option<&str> {
    let t = arg.trim();
    let b = t.as_bytes();
    let q = *b.first()?;
    if b.len() < 2 || !matches!(q, b'"' | b'\'' | b'`') {
        return None;
    }
    let mut j = 1;
    while j < b.len() && b[j] != q {
        j += if b[j] == b'\\' { 2 } else { 1 };
    }
    (j == b.len() - 1).then(|| &t[1..j])
}

/// The literal bound to the first of `names` inside a call body: `name="x"`,
/// `name = 'x'`, `name: "x"`, `"name": "x"` (a Python request dict) or
/// `Name: aws.String("x")` (a Go wrapper call). The name must stand alone —
/// `MySecretId=` does not match `SecretId`.
fn named_literal(window: &str, names: &[&str]) -> Option<String> {
    let b = window.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    for name in names {
        let mut from = 0;
        while let Some(rel) = window[from..].find(name) {
            let at = from + rel;
            from = at + name.len();
            if at > 0 && ident(b[at - 1]) {
                continue;
            }
            let mut i = at + name.len();
            // `"name": x` — a quoted key closes on the quote that opened it.
            if at > 0 && matches!(b[at - 1], b'"' | b'\'') {
                if b.get(i) != Some(&b[at - 1]) {
                    continue;
                }
                i += 1;
            } else if b.get(i).is_some_and(|&c| ident(c)) {
                continue;
            }
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            match (b.get(i), b.get(i + 1)) {
                (Some(b'='), Some(b'=')) | (Some(b':'), Some(b':')) => continue,
                (Some(b'=' | b':'), _) => i += 1,
                _ => continue,
            }
            let value = window[i..].trim_start();
            // An optional wrapper call: `aws.String(` / `&` before the literal.
            let value = value.trim_start_matches('&');
            let value = match value.find('(') {
                Some(p)
                    if p > 0
                        && value[..p]
                            .bytes()
                            .all(|c| ident(c) || c == b'.') =>
                {
                    &value[p + 1..]
                }
                _ => value,
            };
            if let Some(lit) = leading_literal(value) {
                return Some(lit.to_string());
            }
        }
    }
    None
}

/// A string literal at the start of `s` that ends the value: the next
/// non-space byte after it closes the argument (`,` `)` `}` `]`) or the text
/// ends. `"a" + b` is not a whole value.
fn leading_literal(s: &str) -> Option<&str> {
    let s = s.trim_start();
    let b = s.as_bytes();
    let q = *b.first()?;
    if !matches!(q, b'"' | b'\'' | b'`') {
        return None;
    }
    let mut j = 1;
    while j < b.len() && b[j] != q {
        j += if b[j] == b'\\' { 2 } else { 1 };
    }
    if j >= b.len() {
        return None;
    }
    match s[j + 1..].trim_start().bytes().next() {
        None | Some(b',' | b')' | b'}' | b']' | b'\n' | b';') => Some(&s[1..j]),
        _ => None,
    }
}

// ----------------------------------------------------------------------------
// Per-provider normalisers — each returns the ref as the qname will carry it.
// ----------------------------------------------------------------------------

fn plain(s: &str) -> Option<String> {
    Some(s.to_string())
}

/// A Vault path always names `<mount>/<path>`; a slash-less first argument to
/// the generic `.read(` is some other reader.
fn vault_path(s: &str) -> Option<String> {
    let p = s.trim_start_matches('/');
    p.contains('/').then(|| p.to_string())
}

/// A full ARN `arn:aws:secretsmanager:<region>:<acct>:secret:<name>` keeps
/// only the name, so it pairs with a bare-name read of the same secret.
fn aws_ref(s: &str) -> Option<String> {
    match (s.starts_with("arn:"), s.find(":secret:")) {
        (true, Some(i)) => Some(s[i + ":secret:".len()..].to_string()),
        _ => Some(s.to_string()),
    }
}

/// `projects/p/secrets/db/versions/latest` -> `projects/p/secrets/db`: every
/// version of one secret is the same dependency.
fn gcp_ref(s: &str) -> Option<String> {
    Some(match s.find("/versions/") {
        Some(i) => s[..i].to_string(),
        None => s.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::{GRAPH_TYPE, cell_type, edge_category, node_kind};
    use glia_core::CellPayload;

    fn module_id(repo: RepoId) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "test")
    }

    fn qnames(out: &ConfigNodes) -> Vec<String> {
        let mut v: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
        v.sort();
        v
    }

    fn secrets(src: &str) -> ConfigNodes {
        let repo = RepoId(1);
        extract_secret_refs(src, module_id(repo), repo)
    }

    fn flags(src: &str) -> ConfigNodes {
        let repo = RepoId(1);
        extract_feature_flags(src, module_id(repo), repo)
    }

    fn cell_of(out: &ConfigNodes, qname: &str) -> Option<String> {
        let id = out.nav.qname_by_id.iter().find(|(_, q)| *q == qname).map(|(id, _)| *id)?;
        let node = out.nodes.iter().find(|n| n.id == id)?;
        node.cells.iter().find(|c| c.kind == cell_type::ENV).map(|c| match &c.payload {
            CellPayload::Text(s) | CellPayload::Json(s) => s.clone(),
            CellPayload::Bytes(_) => String::new(),
        })
    }

    #[test]
    fn vault_read_path() {
        let out = secrets(
            "import hvac\nc = hvac.Client()\ntoken = c.read(\"secret/data/api\")[\"data\"]\n",
        );
        assert_eq!(qnames(&out), vec!["config:secret:vault/secret/data/api"]);
        assert!(out.edges.iter().all(|e| e.category == edge_category::READS_CONFIG));
        assert_eq!(
            cell_of(&out, "config:secret:vault/secret/data/api").as_deref(),
            Some(r#"{"source":"vault","redacted":true}"#)
        );
    }

    #[test]
    fn vault_read_without_slash_is_not_a_vault_path() {
        // `.read(` is generic: in an hvac file, `parser.read("settings.ini")`
        // is configparser, not a Vault path.
        let out = secrets("import hvac\nparser.read(\"settings.ini\")\n");
        assert!(out.nodes.is_empty());
    }

    #[test]
    fn vault_kv_mount_and_chained_forms() {
        let out = secrets(concat!(
            "import hvac\n",
            "client.secrets.kv.v2.read_secret_version(path='api', mount_point='kv')\n",
        ));
        assert_eq!(qnames(&out), vec!["config:secret:vault/kv/api"]);
        let out = secrets(concat!(
            "import vault \"github.com/hashicorp/vault/api\"\n",
            "s, err := client.KVv2(\"secret\").Get(ctx, \"db\")\n",
            "r, err := client.Logical().Read(\"secret/data/cache\")\n",
        ));
        assert_eq!(
            qnames(&out),
            vec!["config:secret:vault/secret/data/cache", "config:secret:vault/secret/db"]
        );
    }

    #[test]
    fn aws_secret_id_kwarg() {
        let out = secrets(concat!(
            "import boto3\n",
            "sm = boto3.client(\"secretsmanager\")\n",
            "v = sm.get_secret_value(SecretId=\"prod/db-creds\")[\"SecretString\"]\n",
        ));
        assert_eq!(qnames(&out), vec!["config:secret:aws_sm/prod/db-creds"]);
        assert_eq!(
            cell_of(&out, "config:secret:aws_sm/prod/db-creds").as_deref(),
            Some(r#"{"source":"aws_sm","redacted":true}"#)
        );
    }

    #[test]
    fn aws_object_literal_struct_and_initializer_forms() {
        // JS v3: object-literal key, key on its own line, single quotes.
        let js = concat!(
            "import { SecretsManagerClient, GetSecretValueCommand } from '@aws-sdk/client-secrets-manager';\n",
            "const r = await client.send(new GetSecretValueCommand({\n",
            "  SecretId: 'prod/stripe',\n",
            "}));\n",
        );
        assert_eq!(qnames(&secrets(js)), vec!["config:secret:aws_sm/prod/stripe"]);
        // Go: struct field through the `aws.String` wrapper.
        let go = concat!(
            "import \"github.com/aws/aws-sdk-go/service/secretsmanager\"\n",
            "out, err := svc.GetSecretValue(&secretsmanager.GetSecretValueInput{SecretId: aws.String(\"prod/api-key\")})\n",
        );
        assert_eq!(qnames(&secrets(go)), vec!["config:secret:aws_sm/prod/api-key"]);
        // C#: object initializer after `()`; a full ARN keeps only the name.
        let cs = concat!(
            "using Amazon.SecretsManager;\n",
            "var req = new GetSecretValueRequest() { SecretId = \"arn:aws:secretsmanager:eu-west-1:123:secret:prod/queue\" };\n",
        );
        assert_eq!(qnames(&secrets(cs)), vec!["config:secret:aws_sm/prod/queue"]);
    }

    #[test]
    fn gcp_and_azure_secret_names() {
        let py = concat!(
            "from google.cloud import secretmanager\n",
            "r = client.access_secret_version(name=\"projects/p/secrets/db/versions/latest\")\n",
        );
        assert_eq!(qnames(&secrets(py)), vec!["config:secret:gcp_sm/projects/p/secrets/db"]);
        let cs = concat!(
            "using Azure.Security.KeyVault.Secrets;\n",
            "KeyVaultSecret s = await client.GetSecretAsync(\"db-password\");\n",
        );
        assert_eq!(qnames(&secrets(cs)), vec!["config:secret:azure_kv/db-password"]);
    }

    #[test]
    fn rejects_interpolated_ref() {
        let src = concat!(
            "import boto3  # secretsmanager\n",
            "a = sm.get_secret_value(SecretId=f\"{env}/db\")\n",
            "b = sm.get_secret_value(SecretId=\"prod/\" + name)\n",
            "import hvac\n",
            "c = client.read(`secret/${env}/api`)\n",
            "d = client.read(\"secret/%s/api\" % env)\n",
            "e = client.read(\"secret/data/{{ .Values.app }}\")\n",
        );
        assert!(secrets(src).nodes.is_empty(), "{:?}", qnames(&secrets(src)));
    }

    #[test]
    fn secret_needle_needs_its_provider_token() {
        // No hvac / vault token: `.read("a/b")` is some file reader.
        assert!(secrets("data = archive.read(\"docs/readme.md\")\n").nodes.is_empty());
        // No Secrets Manager token: a same-named method on something else.
        assert!(secrets("x = cache.get_secret_value(SecretId=\"prod/x\")\n").nodes.is_empty());
    }

    #[test]
    fn ld_bool_variation_key() {
        let go = concat!(
            "import ld \"github.com/launchdarkly/go-server-sdk/v6\"\n",
            "enabled, _ := client.BoolVariation(\"new-checkout\", ldcontext.New(userKey), false)\n",
        );
        let out = flags(go);
        assert_eq!(qnames(&out), vec!["config:flag:new-checkout"]);
        assert_eq!(
            cell_of(&out, "config:flag:new-checkout").as_deref(),
            Some(r#"{"source":"launchdarkly","redacted":true}"#)
        );
        // TS, single-quoted key, whitespace and a newline before it.
        let ts = concat!(
            "import { init } from 'launchdarkly-node-server-sdk';\n",
            "const on = await ldClient.variation(\n    'dark-mode', { key }, false);\n",
        );
        assert_eq!(qnames(&flags(ts)), vec!["config:flag:dark-mode"]);
    }

    #[test]
    fn unleash_is_enabled_key() {
        let py = concat!(
            "from UnleashClient import UnleashClient\n",
            "if client.is_enabled(\"new_checkout\"):\n    pass\n",
        );
        assert_eq!(qnames(&flags(py)), vec!["config:flag:new_checkout"]);
        let js = "import { initialize } from 'unleash-client';\nif (unleash.isEnabled('beta.search')) {}\n";
        assert_eq!(qnames(&flags(js)), vec!["config:flag:beta.search"]);
    }

    #[test]
    fn openfeature_go_key_follows_ctx_and_split_second_arg() {
        let go = concat!(
            "import \"github.com/open-feature/go-sdk/openfeature\"\n",
            "v, _ := client.BooleanValue(ctx, \"new-search\", false, openfeature.EvaluationContext{})\n",
        );
        assert_eq!(qnames(&flags(go)), vec!["config:flag:new-search"]);
        let js = concat!(
            "import { SplitFactory } from '@splitsoftware/splitio';\n",
            "const t = client.getTreatment(userId, 'checkout_v2');\n",
            "const u = client.getTreatment('browser-flag');\n",
        );
        assert_eq!(
            qnames(&flags(js)),
            vec!["config:flag:browser-flag", "config:flag:checkout_v2"]
        );
    }

    #[test]
    fn rejects_single_char_flag_key() {
        let src = concat!(
            "import ldclient\n",
            "a = ldclient.get().variation(\"a\", ctx, False)\n",
            "b = ldclient.get().variation(\"ab\", ctx, False)\n",
            "c = ldclient.get().variation(flag_name, ctx, False)\n",
        );
        assert!(flags(src).nodes.is_empty(), "{:?}", qnames(&flags(src)));
    }

    #[test]
    fn flag_needle_needs_its_provider_token() {
        // A generic `isEnabled` in a file that never mentions a flag SDK.
        let src = "if (feature.isEnabled(\"x-y\")) { run(); }\n";
        assert!(flags(src).nodes.is_empty());
    }

    #[test]
    fn marker_counts_refs_providers_and_flags() {
        let src = concat!(
            "import boto3, hvac  # secretsmanager\n",
            "a = sm.get_secret_value(SecretId=\"prod/db-creds\")\n",
            "b = c.read(\"secret/data/api\")\n",
        );
        let s = secrets(src);
        let f = flags(src);
        assert_eq!(
            marker(&[&s, &f], "python").as_deref(),
            Some("[secrets] refs=2 providers=2 flags=0 flag_defs=0 src=python")
        );
        assert_eq!(marker(&[&flags("x = 1\n")], "python"), None);
    }
}
