//! Lua plugin families (lua-support): one row per plugin architecture that the
//! parser, the require walk, the manifest route and `resolver:plugin` read.
//!
//! Runtime discovery is the primary source. A repo's families come from
//! `.glia/plugin_families.toml` when a person has written one (created by
//! `glia plugin-families init`, never by a build). Without it, discovery infers
//! a family from the tree: the manifest basename that recurs beside Lua plugin
//! directories, the `lib` roots the Lua sources sit under. [`SEED_FAMILIES`] is
//! the one static table: a set of known architectures that discovery matches
//! against when it infers a family, so adding a future architecture is one row
//! here plus a fixture, never a new code path.
//!
//! Every helper takes its families as a parameter. Nothing here reads the
//! process working directory, holds global state or writes a file; a build
//! passes the repo's [`PluginFamilies`] down explicitly.

use std::collections::BTreeMap;
use std::path::Path;

/// How a registration call carries its name and handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// `f{ name = "x", execute = function() end }`: the first table argument
    /// carries a `name` string field, and anonymous function fields are
    /// handler closures keyed by their field name.
    Table,
    /// `f("x", function() end, { complete = function() end })`: the name is a
    /// positional string argument, handlers are positional anonymous functions,
    /// and a table argument's named keys can carry handlers too.
    Positional {
        /// 1-based argument index of the name (a string literal).
        name_arg: u8,
        /// 1-based argument indexes carrying anonymous function handlers.
        handler_args: Vec<u8>,
        /// `(arg_index, keys)`: a table argument whose named keys carry
        /// handlers under exactly these keys.
        handler_table: Option<(u8, Vec<String>)>,
    },
}

/// The shape tag a detection candidate records (the arguments are the family's
/// business, not the fact's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ShapeTag {
    Table,
    Positional,
}

impl ShapeTag {
    /// The marker's spelling.
    pub fn name(self) -> &'static str {
        match self {
            ShapeTag::Table => "table",
            ShapeTag::Positional => "positional",
        }
    }
}

impl Shape {
    /// The coarse tag a detection candidate records.
    pub fn tag(&self) -> ShapeTag {
        match self {
            Shape::Table => ShapeTag::Table,
            Shape::Positional { .. } => ShapeTag::Positional,
        }
    }
}

/// One registration API of a plugin family: the dotted callee prefix that names
/// its calls, the shape its reader uses, and which suffixes mint PLUGIN_TOOLs
/// (the rest count `aux=`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationCall {
    /// The dotted callee prefix (`maki.api.register_`).
    pub prefix: String,
    /// The shape the reader uses for this API's calls.
    pub shape: Shape,
    /// The callee suffixes (after the matched prefix) that mint a PLUGIN_TOOL.
    /// Empty: every registration in the API mints.
    pub tools: Vec<String>,
    /// The member name a positional handler gets when it is not keyed
    /// (`run`): the handler FUNCTION's qname is `<module>::<tool>.<handler_name>`.
    pub handler_name: String,
}

/// One plugin architecture: the facts its plugins and its loader are spelled
/// with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginFamily {
    /// The family's name, the qname owner segment (`plugin:<dir> @<name>`,
    /// `plugin_tool:<name> @<name>/<dir>`).
    pub name: String,
    /// The manifest basename that marks a plugin directory (`plugin.toml`).
    pub manifest: String,
    /// The registration APIs, in match order (the longest matching prefix wins).
    pub calls: Vec<RegistrationCall>,
    /// The library directories a dotted require walks (`lib`).
    pub lib_roots: Vec<String>,
}

/// The families of one repo, in the order they were declared or discovered.
/// Order is the tie order of every match, so results are deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginFamilies {
    families: Vec<PluginFamily>,
}

impl PluginFamilies {
    /// A set of families, in tie order.
    pub fn new(families: Vec<PluginFamily>) -> Self {
        Self { families }
    }

    /// The families, in tie order.
    pub fn families(&self) -> &[PluginFamily] {
        &self.families
    }

    /// A deterministic fingerprint of the family set, for a caller that keys
    /// cached parses by their parse context (the engine's `ParseContext`):
    /// `name|manifest|prefix…;…` in family order.
    pub fn context_key(&self) -> String {
        self.families
            .iter()
            .map(|f| {
                let mut row = format!("{}|{}|", f.name, f.manifest);
                row.push_str(
                    &f.calls
                        .iter()
                        .map(|c| c.prefix.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                );
                row
            })
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Whether the repo has no family.
    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
    }

    /// The family whose `manifest` the basename of `path` is, if any.
    pub fn manifest_family(&self, path: &str) -> Option<&PluginFamily> {
        manifest_family(path, &self.families)
    }

    /// The family and registration API the dotted callee text belongs to.
    pub fn call_for(&self, dotted_callee: &str) -> Option<(&PluginFamily, &RegistrationCall)> {
        call_for(dotted_callee, &self.families)
    }
}

/// The family whose `manifest` the basename of `path` is, if any.
pub fn manifest_family<'a>(path: &str, families: &'a [PluginFamily]) -> Option<&'a PluginFamily> {
    let base = Path::new(path).file_name()?.to_str()?;
    families.iter().find(|f| f.manifest == base)
}

/// The family and registration API the dotted callee text starts with, if any:
/// the longest matching prefix across all families wins, and a tie keeps the
/// earlier family (table order is the tie order).
pub fn call_for<'a>(
    dotted_callee: &str,
    families: &'a [PluginFamily],
) -> Option<(&'a PluginFamily, &'a RegistrationCall)> {
    let mut best: Option<(&'a PluginFamily, &'a RegistrationCall)> = None;
    for family in families {
        if let Some(call) = call_of(dotted_callee, family) {
            let better = match best {
                None => true,
                Some((_, b)) => call.prefix.len() > b.prefix.len(),
            };
            if better {
                best = Some((family, call));
            }
        }
    }
    best
}

/// The registration API of `family` whose prefix the dotted callee text starts
/// with, if any (longest prefix wins; ties keep the earlier API).
pub fn call_of<'a>(dotted_callee: &str, family: &'a PluginFamily) -> Option<&'a RegistrationCall> {
    let mut best: Option<&'a RegistrationCall> = None;
    for call in &family.calls {
        if dotted_callee.starts_with(call.prefix.as_str()) {
            let better = match best {
                None => true,
                Some(b) => call.prefix.len() > b.prefix.len(),
            };
            if better {
                best = Some(call);
            }
        }
    }
    best
}

/// Whether a registration mints a PLUGIN_TOOL: the suffix after the matched
/// prefix is one of the API's `tools`, or the API lists none (every
/// registration mints). `false` only means "a registration, but not a tool"; it
/// still counts as `aux=`.
pub fn mints_tool(dotted_callee: &str, family: &PluginFamily) -> bool {
    let Some(call) = call_of(dotted_callee, family) else {
        return false;
    };
    let suffix = &dotted_callee[call.prefix.len()..];
    call.tools.is_empty() || call.tools.iter().any(|t| t == suffix)
}

/// The seed architectures: the known plugin families, matched against an
/// inferred family's shape during discovery. A seed is data; the code that reads
/// it is the same for all of them.
pub fn seed_families() -> Vec<PluginFamily> {
    vec![PluginFamily {
        name: "maki".into(),
        manifest: "plugin.toml".into(),
        calls: vec![RegistrationCall {
            prefix: "maki.api.register_".into(),
            shape: Shape::Table,
            tools: vec!["tool".into(), "command".into()],
            handler_name: String::new(),
        }],
        lib_roots: vec!["lib".into()],
    }]
}

/// The families a repo declares in `.glia/plugin_families.toml`, if the file
/// exists and parses. A missing or malformed file yields `None`; discovery then
/// infers from the tree.
pub fn read_declared(repo_root: &Path) -> Option<PluginFamilies> {
    let text = std::fs::read_to_string(repo_root.join(".glia/plugin_families.toml")).ok()?;
    parse_declared(&text)
}

/// Parse a declaration file's text (the `[[plugin_family]]` tables).
pub fn parse_declared(text: &str) -> Option<PluginFamilies> {
    let file: DeclaredFile = toml::de::from_str(text).ok()?;
    Some(PluginFamilies::new(
        file.plugin_family
            .into_iter()
            .map(DeclaredFamily::into_family)
            .collect(),
    ))
}

/// The declared-file text `glia plugin-families init` writes: the commented
/// example, one table per seed family, so the person edits rather than writes
/// from nothing.
pub fn declaration_template() -> String {
    let mut out = String::from(
        "# Lua plugin families for this repo (glia plugin-families).\n\
         # Each [[plugin_family]] names one plugin architecture. The parser, the\n\
         # require walk and the manifest route read these rows; a family absent\n\
         # here is inferred from the tree.\n",
    );
    for family in seed_families() {
        out.push_str("\n[[plugin_family]]\n");
        out.push_str(&format!("name = {:?}\n", family.name));
        out.push_str(&format!("manifest = {:?}\n", family.manifest));
        out.push_str(&format!(
            "lib_roots = [{}]\n",
            family
                .lib_roots
                .iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for call in &family.calls {
            out.push_str("\n[[plugin_family.call]]\n");
            out.push_str(&format!("prefix = {:?}\n", call.prefix));
            out.push_str("shape = \"table\"\n");
            out.push_str(&format!(
                "tools = [{}]\n",
                call.tools
                    .iter()
                    .map(|s| format!("{s:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    out
}

#[derive(serde::Deserialize)]
struct DeclaredFile {
    #[serde(default)]
    plugin_family: Vec<DeclaredFamily>,
}

#[derive(serde::Deserialize)]
struct DeclaredFamily {
    name: String,
    manifest: String,
    #[serde(default)]
    lib_roots: Vec<String>,
    #[serde(default, rename = "call")]
    calls: Vec<DeclaredCall>,
}

#[derive(serde::Deserialize)]
struct DeclaredCall {
    prefix: String,
    #[serde(default)]
    shape: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    handler_name: String,
}

impl DeclaredFamily {
    fn into_family(self) -> PluginFamily {
        PluginFamily {
            name: self.name,
            manifest: self.manifest,
            calls: self
                .calls
                .into_iter()
                .map(DeclaredCall::into_call)
                .collect(),
            lib_roots: self.lib_roots,
        }
    }
}

impl DeclaredCall {
    fn into_call(self) -> RegistrationCall {
        let shape = match self.shape.as_str() {
            "positional" => Shape::Positional {
                name_arg: 1,
                handler_args: vec![2],
                handler_table: None,
            },
            _ => Shape::Table,
        };
        RegistrationCall {
            prefix: self.prefix,
            shape,
            tools: self.tools,
            handler_name: self.handler_name,
        }
    }
}

/// The families a repo builds under: its declared
/// `.glia/plugin_families.toml` when one exists, else what discovery infers
/// from the tree. The one entry point a build reads; pure otherwise.
pub fn for_repo(repo_root: &Path) -> PluginFamilies {
    read_declared(repo_root).unwrap_or_else(|| infer_families(repo_root))
}

/// The manifest basenames the walk admits as candidate plugin manifests: a
/// declared family's manifest when one is declared, else every seed family's
/// manifest (a file admitted but matched by no family costs one small read
/// and routes nowhere). Family-independent on purpose: the walk has no
/// context, so its admission is the union of what a family could name.
pub fn walk_manifest_names(repo_root: &Path) -> Vec<String> {
    match read_declared(repo_root) {
        Some(declared) => {
            let mut names: Vec<String> = declared
                .families
                .iter()
                .map(|f| f.manifest.clone())
                .collect();
            names.sort();
            names.dedup();
            names
        }
        None => {
            let mut names: Vec<String> = seed_families().into_iter().map(|f| f.manifest).collect();
            names.sort();
            names.dedup();
            names
        }
    }
}

/// Infer a family from the tree when no declaration exists: each manifest
/// basename that a seed names, found beside Lua sources, becomes a family with
/// that seed's calls; the `lib` roots are the seed roots the Lua sources sit
/// under. Depth-capped (the seed manifests live at plugin-directory level),
/// ordered by family name, so the result is deterministic.
pub fn infer_families(repo_root: &Path) -> PluginFamilies {
    let mut manifests: BTreeMap<String, usize> = BTreeMap::new();
    let mut lua_dirs: Vec<String> = Vec::new();
    let mut stack = vec![(repo_root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 6 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().and_then(|s| s.to_str()) == Some("lua")
                && let Some(rel) = path.parent().and_then(|p| p.strip_prefix(repo_root).ok())
            {
                lua_dirs.push(rel.to_string_lossy().replace('\\', "/"));
            }
            *manifests.entry(name.to_string()).or_insert(0) += 1;
        }
    }
    let seeds = seed_families();
    let mut out = Vec::new();
    for seed in seeds {
        if manifests.get(&seed.manifest).copied().unwrap_or(0) == 0 || lua_dirs.is_empty() {
            continue;
        }
        let mut roots: Vec<String> = seed.lib_roots.clone();
        // A lib root is kept when Lua sources actually sit under it, at any
        // ancestor level (`plugins/lib/x` keeps `lib`; the require walk tries
        // every ancestor).
        roots.retain(|r| lua_dirs.iter().any(|d| d.split('/').any(|seg| seg == r)));
        out.push(PluginFamily {
            lib_roots: roots,
            ..seed
        });
    }
    PluginFamilies::new(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toy() -> PluginFamily {
        PluginFamily {
            name: "toy".into(),
            manifest: "host.toml".into(),
            calls: vec![RegistrationCall {
                prefix: "toy.register_".into(),
                shape: Shape::Table,
                tools: vec![],
                handler_name: String::new(),
            }],
            lib_roots: vec![],
        }
    }

    #[test]
    fn manifest_matches_basename_only() {
        let fams = seed_families();
        assert!(manifest_family("plugins/question/plugin.toml", &fams).is_some());
        assert!(manifest_family("plugin.toml", &fams).is_some());
        assert!(manifest_family("plugins/plugin.toml/extra.toml", &fams).is_none());
        assert!(manifest_family("plugins/question/other.toml", &fams).is_none());
    }

    #[test]
    fn callee_prefix_is_exact_dotted_prefix() {
        let fams = seed_families();
        assert!(call_for("maki.api.register_tool", &fams).is_some());
        assert!(call_for("maki.api.register_options", &fams).is_some());
        assert!(call_for("maki.api.register_", &fams).is_some());
        assert!(call_for("maki.register_tool", &fams).is_none());
        assert!(call_for("register_tool", &fams).is_none());
        assert!(call_for("tools.api.register_tool", &fams).is_none());
    }

    #[test]
    fn tool_calls_gate_the_mint_not_the_match() {
        let fams = seed_families();
        let (f, _) = call_for("maki.api.register_options", &fams).expect("still a registration");
        assert!(mints_tool("maki.api.register_tool", f));
        assert!(mints_tool("maki.api.register_command", f));
        assert!(!mints_tool("maki.api.register_options", f));
        assert!(!mints_tool("maki.api.register_prompt_hint", f));
        assert!(!mints_tool("maki.api.register_permission_rule", f));
    }

    #[test]
    fn empty_tools_mints_every_registration() {
        let f = toy();
        assert!(mints_tool("toy.register_thing", &f));
        assert!(mints_tool("toy.register_anything_else", &f));
    }

    #[test]
    fn positional_call_drives_call_for_and_mints_tool() {
        let fams = [PluginFamily {
            name: "toy".into(),
            manifest: String::new(),
            calls: vec![RegistrationCall {
                prefix: "toy.on_".into(),
                shape: Shape::Positional {
                    name_arg: 1,
                    handler_args: vec![2],
                    handler_table: None,
                },
                tools: vec![],
                handler_name: "run".into(),
            }],
            lib_roots: vec![],
        }];
        let (f, call) = call_for("toy.on_event", &fams).expect("the positional API matches");
        assert_eq!(f.name, "toy");
        assert_eq!(call.shape.tag(), ShapeTag::Positional);
        assert!(mints_tool("toy.on_event", f));
        assert!(call_for("toy.unrelated", &fams).is_none());
    }

    #[test]
    fn first_matching_family_wins_on_a_tie() {
        let mut second = toy();
        second.name = "toy2".into();
        second.manifest = "host.toml".into();
        let fams = [toy(), second];
        assert_eq!(manifest_family("a/host.toml", &fams).unwrap().name, "toy");
        assert_eq!(call_for("toy.register_x", &fams).unwrap().0.name, "toy");
    }

    #[test]
    fn declaration_round_trips_through_the_template() {
        let parsed = parse_declared(&declaration_template()).expect("the template parses");
        assert_eq!(parsed.families(), seed_families().as_slice());
    }

    #[test]
    fn a_malformed_declaration_is_none() {
        assert!(parse_declared("[[plugin_family]\nname =").is_none());
    }
}
