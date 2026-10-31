//! The host table (0.5.6, GH.2): the programs that load code and call into it
//! — editors, games, web frameworks, test runners, bundlers — and how a
//! plugin registers with each. Generalised from the plugin families of Brian
//! Ealdwine's glia PR #3 (its `code-domain/src/plugin_families.rs`: a family
//! table, `call_for`'s longest-prefix registration lookup, and the Maki row
//! as its seed), credited with thanks.
//!
//! A row says WHAT to look for, never that something is there (James
//! 2026-10-31: "as long as it's not a crutch"). No row mints an edge by
//! itself: a parser reads the registration call in the AST — the callee, a
//! string-literal name and the handler argument — and only where the host is
//! present (here: the callee is rooted at the host's own global, unshadowed
//! by a local). The parser then calls [`push_command`](crate::hosts::push_command) / [`push_event`](crate::hosts::push_event),
//! which mint the node and its HANDLED_BY [`UnresolvedRef`](crate::UnresolvedRef)s; the graph
//! builder binds those as it binds any handler ref.
//!
//! | Host | Registration | Node |
//! |---|---|---|
//! | `maki` | `maki.api.register_tool{ name = "t", execute = .., restore = .. }` (and `register_command`) | CLI_COMMAND `host_cmd:maki:t` HANDLED_BY execute / restore |
//! | `neovim` | `vim.api.nvim_create_user_command("Name", handler, opts)`, `vim.api.nvim_buf_create_user_command(buf, "Name", handler, opts)` | CLI_COMMAND `host_cmd:neovim:Name` HANDLED_BY the handler |
//!
//! | `wezterm` (config, GH.5) | in a file requiring `wezterm`: `local c = wezterm.config_builder()` then `c.<key> = v`; or the table the chunk returns (`return { <key> = v }`, or a chunk-level `local c = {}` it returns) | CONFIG_KEY `config:wezterm:<key>`, DEFINES_CONFIG from the MODULE |
//! | `neovim` (config, GH.5) | `vim.opt` / `vim.opt_local` / `vim.opt_global` / `vim.o` / `vim.go` / `vim.bo` / `vim.wo` `.<key> = v` (or `:append` / `:prepend` / `:remove`), `vim.g.<key> = v` | CONFIG_KEY `config:neovim:<key>` (`config:neovim:g.<key>`), DEFINES_CONFIG from the MODULE; an rvalue read is READS_CONFIG from the enclosing scope |
//!
//! Only a setting's key is a node, never its value. A host's settings are
//! its own: [`split_host_config_qname`](crate::hosts::split_host_config_qname)
//! names them, and the config resolver never pairs one across repos (two
//! dotfiles repos setting `font_size` share nothing).
//!
//! Other `maki.api.register_*` calls match the Maki row but mint nothing
//! (counted `aux=`, PR #3's ruling). Neovim keymaps are not commands (James
//! 2026-10-31): `vim.keymap.set` matches no row.
//!
//! Qnames are host-scoped, not file-scoped: `host_cmd:<host>:<name>` and
//! `host_event:<host>:<event>`, so a command two files register is one node.
//! No registry id is new: commands are CLI_COMMAND, events EVENT_HANDLER.
//!
//! fired_on, per file holding a registration ([`HostTally::marker`](crate::hosts::HostTally::marker)):
//! `[host] lang=<l> commands=<c> events=<e> callbacks=<k> config=<g> minted=<m> aux=<a> unread=<u> hosts=<h:n,..> file=<p>`.
//!
//! **The process layer (GH.14).** Environment hosts (a Dockerfile, compose,
//! Kubernetes, a Procfile, systemd) each declare an argv, and the same
//! launchers recur under all of them. [`LAUNCHERS`](crate::hosts::LAUNCHERS) holds one row per
//! launcher (`python`, `node`, `gunicorn`, `celery`, `yarn`, `java`, `dotnet`,
//! the wrappers `tini --` / `gosu <user>` / `env K=V` / `bundle exec` / `npx`,
//! `sh -c`, the framework runners), and [`normalise_argv`](crate::hosts::normalise_argv) reads an argv
//! through its assignments, wrappers and `sh -c` strings (at most
//! [`MAX_LAUNCH_DEPTH`](crate::hosts::MAX_LAUNCH_DEPTH) hops) to a [`StartTarget`](crate::hosts::StartTarget): how the started target is
//! named ([`TargetForm`](crate::hosts::TargetForm)) and the name, or why none can be stood behind
//! ([`Refusal`](crate::hosts::Refusal): `dynamic`, `inline`, `depth`, `no_target`). A table and
//! one pure function: it mints nothing and prints nothing; its readers (the
//! host-start resolver) print the form counts in `[host-start]`.

use std::collections::BTreeMap;

use glia_core::{Cell, Confidence, Edge, Node, NodeId, NodeKindId, RepoId};

use crate::evidence::Evidence;
use crate::{CallQualifier, CodeNav, GRAPH_TYPE, UnresolvedRef, edge_category, node_kind};

/// One host: the program that loads plugin code and calls into it.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct HostRow {
    /// The host's name, the `<host>` segment of its qnames (`neovim`).
    pub name: &'static str,
    /// The parser that reads its registrations (`lua`).
    pub lang: &'static str,
    /// The file extensions its plugin code is written in, without the dot.
    pub exts: &'static [&'static str],
    /// What makes the host present in a file.
    pub gate: Gate,
    /// Where the host loads plugin units from: repo-relative path globs. Read
    /// by the plugin-unit load rows (none of the rows here declares one).
    pub loads: &'static [&'static str],
    /// The registration calls a plugin makes.
    pub registrations: &'static [Registration],
    /// The functions the host calls by name (`love.update`). None of the rows
    /// here declares one.
    pub callbacks: &'static [Callback],
    /// The settings a plugin or user config writes for the host. None of the
    /// rows here declares one.
    pub config: &'static [ConfigRule],
}

/// What makes a host present in a file. A registration's callee is read only
/// when the gate holds where the call is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Gate {
    /// The host injects a global (`vim`, `maki`): a callee rooted at that
    /// name, which no local of the call's scope shadows (a local bound to the
    /// global itself or to one of its fields, `local api = vim.api`, reads
    /// through to it).
    Global(&'static str),
    /// The host is a module the file requires (`require("wezterm")`): a
    /// local bound to that require roots the host's paths (`local w =
    /// require("wezterm")` makes `w.config_builder` the host's
    /// `wezterm.config_builder`). A global of the module's name roots
    /// nothing: only the require binds it.
    Require(&'static str),
}

impl Gate {
    /// The root every path of the row starts with: the global, or the
    /// required module's name.
    pub fn root(self) -> &'static str {
        match self {
            Gate::Global(g) | Gate::Require(g) => g,
        }
    }
}

/// One registration call of a host.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Registration {
    /// The callee, dotted, rooted at the gate's global.
    pub callee: Callee,
    /// Whether it registers a command or an event handler.
    pub pattern: Pattern,
    /// Where the call's name and handler are.
    pub shape: Shape,
}

/// How a registration's callee is matched against a call's dotted callee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Callee {
    /// The whole dotted callee (`vim.api.nvim_create_user_command`).
    Exact(&'static str),
    /// A prefix of the callee's text whose remainder (the suffix) is one
    /// identifier tail (`maki.api.register_` + `tool`). Only a suffix in
    /// `mint` registers a node; any other is counted `aux=`.
    Prefix {
        prefix: &'static str,
        mint: &'static [&'static str],
    },
}

impl Callee {
    /// The callee's text: the exact name or the prefix.
    pub fn text(&self) -> &'static str {
        match self {
            Callee::Exact(t) => t,
            Callee::Prefix { prefix, .. } => prefix,
        }
    }
}

/// What a registration declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Pattern {
    /// A command the host's user invokes by name: CLI_COMMAND
    /// `host_cmd:<host>:<name>`.
    Command,
    /// A handler the host calls when the named event fires: EVENT_HANDLER
    /// `host_event:<host>:<event>`.
    Event,
}

impl Pattern {
    /// The key an anonymous handler is minted under in its file: `command
    /// <name>` / `on <name>` for the registration's first handler, with the
    /// handler's table key appended for any other (`command t restore`).
    pub fn handler_key(self, name: &str, field: Option<&str>) -> String {
        let head = match self {
            Pattern::Command => "command",
            Pattern::Event => "on",
        };
        match field {
            Some(f) => format!("{head} {name} {f}"),
            None => format!("{head} {name}"),
        }
    }

    /// The node kind the pattern mints.
    pub fn kind(self) -> NodeKindId {
        match self {
            Pattern::Command => node_kind::CLI_COMMAND,
            Pattern::Event => node_kind::EVENT_HANDLER,
        }
    }
}

/// Where a registration call holds its name and handler. Argument positions
/// are 1-based, as the call is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Shape {
    /// One table argument: the name is the string at `name_key`, each
    /// `handler_keys` field present is a handler (the first one present is
    /// the registration's primary handler).
    Table {
        name_key: &'static str,
        handler_keys: &'static [&'static str],
    },
    /// Positional arguments: the name is the string at `name_arg`; the
    /// handler is the argument at `handler_arg`, or, with `handler_table`
    /// `Some((arg, keys))`, the `keys` fields of the table at `arg`.
    Positional {
        name_arg: usize,
        handler_arg: usize,
        handler_table: Option<(usize, &'static [&'static str])>,
    },
}

/// Functions a host calls by name: `<prefix><name>` for each of `names`.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Callback {
    pub prefix: &'static str,
    pub names: &'static [&'static str],
}

/// How a host's settings are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigFlavour {
    /// `<prefix>.<key> = v` (and the prefix's [`CONFIG_MUTATORS`] calls,
    /// `<prefix>.<key>:append(..)`); a read of `<prefix>.<key>` elsewhere is
    /// a READS_CONFIG. The prefix is a live host global.
    Assign,
    /// The keys of the table a `<prefix>` builder call returns, set on the
    /// local bound to it (`c.<key> = v`). The host reads the table the chunk
    /// returns, so in a file whose gate holds, the keys of a returned table
    /// literal (or of a chunk-level local bound to one and returned) are its
    /// settings too. A function value is never a setting.
    Builder,
}

impl ConfigFlavour {
    /// `assign` / `builder`, the flavour's EVIDENCE rule suffix.
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigFlavour::Assign => "assign",
            ConfigFlavour::Builder => "builder",
        }
    }
}

/// One way a host's settings are written: a flavour and its prefixes.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ConfigRule {
    pub flavour: ConfigFlavour,
    /// Dotted, rooted at the gate's root (`vim.opt`, `wezterm.config_builder`).
    pub prefixes: &'static [&'static str],
    /// The prefixes (each one of `prefixes`) whose keys are a namespace of
    /// their own, qualified by the prefix's last segment: `vim.g.leader` is
    /// `g.leader`, a variable, never the option `leader`.
    pub namespaced: &'static [&'static str],
}

/// The method calls on an [`ConfigFlavour::Assign`] setting that write it
/// (Neovim's option objects: `vim.opt.shortmess:append("c")`).
pub const CONFIG_MUTATORS: &[&str] = &["append", "prepend", "remove"];

/// The host table. Order matters: on a tie in [`registration_for`] the
/// earlier row wins. Maki first (PR #3's seed).
pub static HOSTS: &[HostRow] = &[
    HostRow {
        name: "maki",
        lang: "lua",
        exts: &["lua"],
        gate: Gate::Global("maki"),
        loads: &[],
        registrations: &[Registration {
            callee: Callee::Prefix {
                prefix: "maki.api.register_",
                mint: &["tool", "command"],
            },
            pattern: Pattern::Command,
            shape: Shape::Table {
                name_key: "name",
                handler_keys: &["execute", "restore"],
            },
        }],
        callbacks: &[],
        config: &[],
    },
    HostRow {
        name: "neovim",
        lang: "lua",
        exts: &["lua"],
        gate: Gate::Global("vim"),
        loads: &[],
        registrations: &[
            Registration {
                callee: Callee::Exact("vim.api.nvim_create_user_command"),
                pattern: Pattern::Command,
                shape: Shape::Positional {
                    name_arg: 1,
                    handler_arg: 2,
                    handler_table: None,
                },
            },
            Registration {
                callee: Callee::Exact("vim.api.nvim_buf_create_user_command"),
                pattern: Pattern::Command,
                shape: Shape::Positional {
                    name_arg: 2,
                    handler_arg: 3,
                    handler_table: None,
                },
            },
        ],
        callbacks: &[],
        config: &[ConfigRule {
            flavour: ConfigFlavour::Assign,
            prefixes: &[
                "vim.opt",
                "vim.opt_local",
                "vim.opt_global",
                "vim.o",
                "vim.go",
                "vim.bo",
                "vim.wo",
                "vim.g",
            ],
            namespaced: &["vim.g"],
        }],
    },
    HostRow {
        name: "wezterm",
        lang: "lua",
        exts: &["lua"],
        gate: Gate::Require("wezterm"),
        loads: &[],
        registrations: &[],
        callbacks: &[],
        config: &[ConfigRule {
            flavour: ConfigFlavour::Builder,
            prefixes: &["wezterm.config_builder"],
            namespaced: &[],
        }],
    },
];

/// A registration a dotted callee matched.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RegistrationMatch<'d> {
    pub host: &'static HostRow,
    pub registration: &'static Registration,
    /// The callee's remainder after a [`Callee::Prefix`] (`tool`); empty for
    /// an exact callee.
    pub suffix: &'d str,
    /// False for a prefix match whose suffix is not in its `mint` list: the
    /// call is the host's, counted `aux=`, and registers no node.
    pub mints: bool,
}

/// The registration a call's dotted callee (expanded to its host global:
/// `vim.api.nvim_create_user_command`) matches among `lang`'s rows: the
/// longest matching callee wins, table order breaks a tie (PR #3's
/// `call_for`). `None` when no row of `lang` matches.
pub fn registration_for<'d>(lang: &str, dotted: &'d str) -> Option<RegistrationMatch<'d>> {
    let mut best: Option<(usize, RegistrationMatch<'d>)> = None;
    for host in HOSTS.iter().filter(|h| h.lang == lang) {
        for registration in host.registrations {
            let found = match registration.callee {
                Callee::Exact(c) => (dotted == c).then_some(("", true)),
                Callee::Prefix { prefix, mint } => dotted
                    .strip_prefix(prefix)
                    .filter(|rest| is_identifier_tail(rest))
                    .map(|rest| (rest, mint.contains(&rest))),
            };
            let Some((suffix, mints)) = found else {
                continue;
            };
            let len = registration.callee.text().len();
            if best.as_ref().is_none_or(|(l, _)| len > *l) {
                best = Some((
                    len,
                    RegistrationMatch {
                        host,
                        registration,
                        suffix,
                        mints,
                    },
                ));
            }
        }
    }
    best.map(|(_, m)| m)
}

/// True when `name` is the global a host row of `lang` is gated on.
pub fn is_host_global(lang: &str, name: &str) -> bool {
    HOSTS
        .iter()
        .any(|h| h.lang == lang && matches!(h.gate, Gate::Global(g) if g == name))
}

/// The row of `lang` gated on requiring `spec` (`require("wezterm")`):
/// the local bound to that require roots the row's paths.
pub fn host_for_require(lang: &str, spec: &str) -> Option<&'static HostRow> {
    HOSTS
        .iter()
        .find(|h| h.lang == lang && matches!(h.gate, Gate::Require(r) if r == spec))
}

/// A setting a host path names.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ConfigMatch {
    pub host: &'static HostRow,
    pub flavour: ConfigFlavour,
    /// The setting's key: `font_size`, or `g.mapleader` under a namespaced
    /// prefix.
    pub key: String,
}

impl ConfigMatch {
    /// A setting of `host` read by the parser itself (a returned table's key).
    pub fn new(host: &'static HostRow, flavour: ConfigFlavour, key: impl Into<String>) -> Self {
        Self {
            host,
            flavour,
            key: key.into(),
        }
    }
}

/// The setting a dotted host path names among `lang`'s rows: an
/// [`ConfigFlavour::Assign`] prefix and one identifier
/// (`vim.opt.number`), or a [`ConfigFlavour::Builder`] prefix's call and one
/// identifier (`wezterm.config_builder().font_size`, the path the parser
/// gives a local bound to the builder call). The longest prefix wins, table
/// order breaks a tie. `None` for a deeper path (`vim.opt.x.y`), a bare
/// prefix or any other path.
pub fn config_for(lang: &str, path: &str) -> Option<ConfigMatch> {
    let mut best: Option<(usize, ConfigMatch)> = None;
    for host in HOSTS.iter().filter(|h| h.lang == lang) {
        for rule in host.config {
            for prefix in rule.prefixes {
                let rest = match rule.flavour {
                    ConfigFlavour::Assign => path.strip_prefix(prefix),
                    ConfigFlavour::Builder => {
                        path.strip_prefix(prefix).and_then(|r| r.strip_prefix("()"))
                    }
                };
                let Some(key) = rest
                    .and_then(|r| r.strip_prefix('.'))
                    .filter(|k| is_identifier_tail(k))
                else {
                    continue;
                };
                if best.as_ref().is_some_and(|(l, _)| prefix.len() <= *l) {
                    continue;
                }
                let key = if rule.namespaced.contains(prefix) {
                    let ns = prefix.rsplit('.').next().unwrap_or(prefix);
                    format!("{ns}.{key}")
                } else {
                    key.to_string()
                };
                best = Some((
                    prefix.len(),
                    ConfigMatch {
                        host,
                        flavour: rule.flavour,
                        key,
                    },
                ));
            }
        }
    }
    best.map(|(_, m)| m)
}

/// The host whose [`ConfigFlavour::Builder`] prefix is exactly `callee` (a
/// call's dotted callee expanded to its host root): a local bound to the call
/// holds that host's settings.
pub fn config_builder_for(lang: &str, callee: &str) -> Option<&'static HostRow> {
    HOSTS.iter().filter(|h| h.lang == lang).find(|h| {
        h.config
            .iter()
            .any(|r| r.flavour == ConfigFlavour::Builder && r.prefixes.contains(&callee))
    })
}

/// The host that reads the table a chunk returns: the first row of `lang`
/// gated on requiring one of `required` and declaring a
/// [`ConfigFlavour::Builder`] rule.
pub fn returned_config_host<'r>(
    lang: &str,
    required: impl Iterator<Item = &'r str>,
) -> Option<&'static HostRow> {
    let specs: Vec<&str> = required.collect();
    HOSTS.iter().find(|h| {
        h.lang == lang
            && h.config.iter().any(|r| r.flavour == ConfigFlavour::Builder)
            && matches!(h.gate, Gate::Require(r) if specs.contains(&r))
    })
}

/// `config:<host>:<key>`.
pub fn host_config_qname(host: &str, key: &str) -> String {
    format!("config:{host}:{key}")
}

/// `(host, key)` of a `config:<host>:<key>` qname whose `<host>` is a
/// [`HOSTS`] row's name; `None` for any other qname (`config:env:..`).
pub fn split_host_config_qname(qname: &str) -> Option<(&'static str, &str)> {
    let (segment, key) = qname.strip_prefix("config:")?.split_once(':')?;
    let host = HOSTS.iter().find(|h| h.name == segment)?;
    Some((host.name, key))
}

/// Which side of a setting a site is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigSide {
    /// Written: DEFINES_CONFIG.
    Define,
    /// Read: READS_CONFIG.
    Read,
}

/// Mint the CONFIG_KEY [`host_config_qname`]`(found.host, found.key)` (once
/// per file; POSITION from `site`, its first site, contained by the site's
/// MODULE as an env key is) and push one DEFINES_CONFIG / READS_CONFIG edge
/// from `from`, unless `edges` already holds that edge. Its EVIDENCE is the
/// host's parser's (`parser:<lang>`), rule `host_config_<flavour>`
/// (`host_config_read` for a read), at the site's line. Returns the key's id.
pub fn push_config_key(
    site: &HostSite<'_>,
    found: &ConfigMatch,
    side: ConfigSide,
    from: NodeId,
    sink: &mut HostSink<'_>,
    edges: &mut Vec<Edge>,
) -> NodeId {
    let key = found.key.as_str();
    let qname = host_config_qname(found.host.name, key);
    let kind = node_kind::CONFIG_KEY;
    let id = NodeId::from_parts(GRAPH_TYPE, site.repo, kind, &qname);
    if !sink.nav.qname_by_id.contains_key(&id) {
        sink.nodes.push(Node {
            id,
            repo: site.repo,
            confidence: Confidence::Strong,
            cells: site.cells.clone(),
        });
        sink.nav.record(id, key, &qname, kind, Some(site.module_id));
    }
    let category = match side {
        ConfigSide::Define => edge_category::DEFINES_CONFIG,
        ConfigSide::Read => edge_category::READS_CONFIG,
    };
    if !edges
        .iter()
        .any(|e| e.from == from && e.to == id && e.category == category)
    {
        let rule = match side {
            ConfigSide::Define => format!("host_config_{}", found.flavour.as_str()),
            ConfigSide::Read => "host_config_read".to_string(),
        };
        let ev = Evidence::emitter(format!("parser:{}", found.host.lang))
            .rule(rule)
            .line(site.line);
        edges.push(Edge::new(from, id, category, Confidence::Strong).with_cell(ev.to_cell()));
    }
    id
}

fn is_identifier_tail(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `host_cmd:<host>:<name>`.
pub fn host_command_qname(host: &str, name: &str) -> String {
    format!("host_cmd:{host}:{name}")
}

/// `host_event:<host>:<event>`.
pub fn host_event_qname(host: &str, event: &str) -> String {
    format!("host_event:{host}:{event}")
}

/// `(host, name)` of a `host_cmd:` / `host_event:` qname; `None` for any
/// other qname.
pub fn split_host_qname(qname: &str) -> Option<(&str, &str)> {
    let rest = qname
        .strip_prefix("host_cmd:")
        .or_else(|| qname.strip_prefix("host_event:"))?;
    rest.split_once(':')
}

/// Where a registration was read: its repo, its file's MODULE, its host, its
/// 0-based row and the cells the minted node carries (its POSITION).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct HostSite<'a> {
    pub repo: RepoId,
    pub module_id: NodeId,
    pub host: &'a str,
    pub line: u32,
    pub cells: Vec<Cell>,
}

impl<'a> HostSite<'a> {
    pub fn new(
        repo: RepoId,
        module_id: NodeId,
        host: &'a str,
        line: u32,
        cells: Vec<Cell>,
    ) -> Self {
        Self {
            repo,
            module_id,
            host,
            line,
            cells,
        }
    }
}

/// The parse output a registration writes into.
#[non_exhaustive]
pub struct HostSink<'a> {
    pub nodes: &'a mut Vec<Node>,
    pub nav: &'a mut CodeNav,
    pub refs: &'a mut Vec<UnresolvedRef>,
}

impl<'a> HostSink<'a> {
    pub fn new(
        nodes: &'a mut Vec<Node>,
        nav: &'a mut CodeNav,
        refs: &'a mut Vec<UnresolvedRef>,
    ) -> Self {
        Self { nodes, nav, refs }
    }
}

/// Mint the CLI_COMMAND `host_cmd:<host>:<name>` (once per file: a second
/// registration of the name adds only its refs) and one HANDLED_BY ref to
/// each handler. The node has no parent: it is the host's, not the file's,
/// so it never shadows a same-named function of its module when a Bare
/// handler ref is bound.
pub fn push_command(
    site: &HostSite<'_>,
    name: &str,
    handlers: &[CallQualifier],
    sink: &mut HostSink<'_>,
) -> NodeId {
    push_registration(site, Pattern::Command, name, handlers, sink)
}

/// Mint the EVENT_HANDLER `host_event:<host>:<event>` and its HANDLED_BY
/// refs, as [`push_command`].
pub fn push_event(
    site: &HostSite<'_>,
    event: &str,
    handlers: &[CallQualifier],
    sink: &mut HostSink<'_>,
) -> NodeId {
    push_registration(site, Pattern::Event, event, handlers, sink)
}

/// [`push_command`] or [`push_event`], by `pattern`: what a parser calls with
/// the pattern of the [`Registration`] it read.
pub fn push_registration(
    site: &HostSite<'_>,
    pattern: Pattern,
    name: &str,
    handlers: &[CallQualifier],
    sink: &mut HostSink<'_>,
) -> NodeId {
    let qname = match pattern {
        Pattern::Command => host_command_qname(site.host, name),
        Pattern::Event => host_event_qname(site.host, name),
    };
    let kind = pattern.kind();
    let id = NodeId::from_parts(GRAPH_TYPE, site.repo, kind, &qname);
    if !sink.nav.qname_by_id.contains_key(&id) {
        sink.nodes.push(Node {
            id,
            repo: site.repo,
            confidence: Confidence::Strong,
            cells: site.cells.clone(),
        });
        sink.nav.record(id, name, &qname, kind, None);
    }
    for handler in handlers {
        sink.refs.push(UnresolvedRef {
            from: id,
            from_module: site.module_id,
            qualifier: handler.clone(),
            category: edge_category::HANDLED_BY,
            line: site.line,
        });
    }
    id
}

/// One file's registration counts, for the `[host]` marker.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct HostTally {
    /// Command registrations read (name literal), node minted.
    pub commands: usize,
    /// Event registrations read, node minted.
    pub events: usize,
    /// Host callbacks recognised.
    pub callbacks: usize,
    /// Host setting sites read (written or read), one per key per site.
    pub config: usize,
    /// Anonymous handlers minted as FUNCTIONs.
    pub minted: usize,
    /// Host calls that matched a prefix row but register nothing.
    pub aux: usize,
    /// Registrations whose name is not a string literal: nothing minted.
    pub unread: usize,
    /// Matched registrations (read, aux and unread) and setting sites per
    /// host.
    pub hosts: BTreeMap<&'static str, usize>,
}

impl HostTally {
    /// Count one matched registration of `host`.
    pub fn hit(&mut self, host: &'static str) {
        *self.hosts.entry(host).or_insert(0) += 1;
    }

    /// Count one registration read and minted, by its pattern.
    pub fn registered(&mut self, pattern: Pattern) {
        match pattern {
            Pattern::Command => self.commands += 1,
            Pattern::Event => self.events += 1,
        }
    }

    /// `[host] lang=.. file=<p>`, or `None` when the file matched no row.
    pub fn marker(&self, lang: &str, file: &str) -> Option<String> {
        if self.hosts.is_empty() && self.callbacks == 0 && self.config == 0 {
            return None;
        }
        let hosts: Vec<String> = self.hosts.iter().map(|(h, n)| format!("{h}:{n}")).collect();
        Some(format!(
            "[host] lang={lang} commands={} events={} callbacks={} config={} minted={} aux={} unread={} hosts={} file={file}",
            self.commands,
            self.events,
            self.callbacks,
            self.config,
            self.minted,
            self.aux,
            self.unread,
            hosts.join(","),
        ))
    }
}

/// Host names a config key segment could be confused with: `config:env:..`
/// and its siblings are other readers' namespaces.
const RESERVED_NAMES: &[&str] = &["env", "secret", "flag", "setting"];

/// Every problem in `rows`: duplicate or reserved host names, empty
/// language / extensions / callees, a callee not rooted at its row's gate
/// root, a prefix callee with nothing to mint, a shape with an empty key or
/// a 0 / shared argument position, empty load globs or callback names, and a
/// config rule with no prefix, a prefix not rooted at the gate's root or a
/// namespaced prefix it does not declare. Empty = valid.
pub fn validate_rows(rows: &[HostRow]) -> Vec<String> {
    let mut problems = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let at = format!("host {:?}", row.name);
        if row.name.is_empty() || !is_identifier_tail(row.name) {
            problems.push(format!("{at}: name must be a non-empty identifier"));
        }
        if RESERVED_NAMES.contains(&row.name) {
            problems.push(format!("{at}: name is reserved"));
        }
        if rows[..i].iter().any(|r| r.name == row.name) {
            problems.push(format!("{at}: duplicate name"));
        }
        if row.lang.is_empty()
            || row.exts.is_empty()
            || row.exts.iter().any(|e| e.is_empty() || e.starts_with('.'))
        {
            problems.push(format!(
                "{at}: lang and exts must be set, exts without dots"
            ));
        }
        let global = row.gate.root();
        if global.is_empty() {
            problems.push(format!("{at}: empty gate root"));
        }
        for r in row.registrations {
            let callee = r.callee.text();
            if callee.is_empty() || !callee.starts_with(&format!("{global}.")) {
                problems.push(format!(
                    "{at}: callee {callee:?} is not rooted at `{global}.`"
                ));
            }
            if let Callee::Prefix { mint, .. } = r.callee
                && (mint.is_empty() || mint.iter().any(|m| !is_identifier_tail(m)))
            {
                problems.push(format!(
                    "{at}: prefix callee {callee:?} needs identifier mint suffixes"
                ));
            }
            match r.shape {
                Shape::Table {
                    name_key,
                    handler_keys,
                } => {
                    if name_key.is_empty()
                        || handler_keys.is_empty()
                        || handler_keys.iter().any(|k| k.is_empty() || *k == name_key)
                    {
                        problems.push(format!("{at}: table shape of {callee:?} needs a name key and distinct handler keys"));
                    }
                }
                Shape::Positional {
                    name_arg,
                    handler_arg,
                    handler_table,
                } => {
                    let table_arg = handler_table.map(|(a, _)| a);
                    if name_arg == 0
                        || handler_arg == 0
                        || name_arg == handler_arg
                        || table_arg == Some(0)
                        || table_arg == Some(name_arg)
                        || handler_table.is_some_and(|(_, keys)| keys.is_empty())
                    {
                        problems.push(format!(
                            "{at}: positional shape of {callee:?} needs distinct 1-based arguments"
                        ));
                    }
                }
            }
        }
        if row.loads.iter().any(|l| l.is_empty()) {
            problems.push(format!("{at}: empty load glob"));
        }
        if row.callbacks.iter().any(|c| {
            c.prefix.is_empty() || c.names.is_empty() || c.names.iter().any(|n| n.is_empty())
        }) {
            problems.push(format!("{at}: a callback rule needs a prefix and names"));
        }
        for (j, c) in row.config.iter().enumerate() {
            if c.prefixes.is_empty() || c.prefixes.iter().any(|p| p.is_empty()) {
                problems.push(format!(
                    "{at}: a {:?} config rule needs prefixes",
                    c.flavour
                ));
            }
            if let Some(p) = c
                .prefixes
                .iter()
                .find(|p| !p.is_empty() && !p.starts_with(&format!("{global}.")))
            {
                problems.push(format!(
                    "{at}: config prefix {p:?} is not rooted at `{global}.`"
                ));
            }
            if let Some(n) = c.namespaced.iter().find(|n| !c.prefixes.contains(n)) {
                problems.push(format!(
                    "{at}: namespaced prefix {n:?} is not one of its rule's prefixes"
                ));
            }
            if row.config[..j].iter().any(|o| o.flavour == c.flavour) {
                problems.push(format!(
                    "{at}: {:?} config flavour declared twice",
                    c.flavour
                ));
            }
        }
    }
    problems
}

/// [`validate_rows`] over [`HOSTS`].
pub fn validate() -> Vec<String> {
    validate_rows(HOSTS)
}

// ---------------------------------------------------------------------------
// The process layer (GH.14): LAUNCHERS and the one argv normaliser.
// ---------------------------------------------------------------------------

/// How a launcher names the target it starts. A [`Launcher`] row's form is how
/// its reader starts; the [`StartTarget`]'s form is what the reader found
/// (the `python` row is [`TargetForm::Script`] and reads `python -m a.b` as
/// [`TargetForm::PyModule`]). The `<host>:<form>` rule names read
/// [`TargetForm::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum TargetForm {
    /// The first non-flag argument is a source file (`python email_server.py`,
    /// `node server.js`, `sh start.sh`).
    Script,
    /// `python -m a.b`: a module path.
    PyModule,
    /// A WSGI / ASGI server's first `a.b:c` argument (`uvicorn server:app`):
    /// target `a.b`, attr `c`.
    ModuleAttr,
    /// `celery -A|--app a.b[:x] worker|beat`.
    CeleryApp,
    /// `flask --app x run` (or `FLASK_APP=x flask run`).
    FlaskApp,
    /// A package-manager script name (`npm run x`, `yarn x`, `pnpm x`,
    /// `deno task x`), looked up in the package manifest by a later reader.
    PkgScript,
    /// `java [-opts] -jar x.jar`.
    Jar,
    /// `java -cp .. a.b.Main` (or `-m mod/a.b.Main`): a main class.
    JavaMain,
    /// `dotnet X.dll` (or `dotnet run --project P`).
    Dll,
    /// A launcher that starts its argument as a command (`tini --`, `gosu
    /// <user>`, `env K=V`, `bundle exec`, `npx`): stripped and re-read.
    Wrapper,
    /// `sh|bash [-l]c "<str>"`: the string is split with shell-word rules and
    /// the last simple command of its `&&` / `;` chain is re-read.
    Shell,
    /// A framework's own runner (`rails`, `puma`, `sidekiq`, `manage.py`):
    /// recorded for the app layer to bind.
    Framework,
    /// Anything else: `argv[0]` itself.
    Binary,
}

impl TargetForm {
    /// The lowercase name a rule and a marker print (`py_module`).
    pub fn as_str(self) -> &'static str {
        match self {
            TargetForm::Script => "script",
            TargetForm::PyModule => "py_module",
            TargetForm::ModuleAttr => "module_attr",
            TargetForm::CeleryApp => "celery_app",
            TargetForm::FlaskApp => "flask_app",
            TargetForm::PkgScript => "pkg_script",
            TargetForm::Jar => "jar",
            TargetForm::JavaMain => "java_main",
            TargetForm::Dll => "dll",
            TargetForm::Wrapper => "wrapper",
            TargetForm::Shell => "shell",
            TargetForm::Framework => "framework",
            TargetForm::Binary => "binary",
        }
    }
}

/// Why [`normalise_argv`] named no target it can stand behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Refusal {
    /// The target is a runtime value (`$APP`, `${X}`, `"$@"`, a command
    /// substitution) or the shell string holds what a line scanner does not
    /// read (a heredoc, a function, a conditional or loop, a subshell, an
    /// unterminated quote).
    Dynamic,
    /// The code is inline (`python -c`, `node -e`, `-` for stdin).
    Inline,
    /// More than [`MAX_LAUNCH_DEPTH`] wrappers and shells deep.
    Depth,
    /// The launcher names no target (`exec` alone, `celery worker` with no
    /// `-A`, an empty argv).
    NoTarget,
}

impl Refusal {
    /// The lowercase reason a marker and an unresolved count print.
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::Dynamic => "dynamic",
            Refusal::Inline => "inline",
            Refusal::Depth => "depth",
            Refusal::NoTarget => "no_target",
        }
    }
}

/// One launcher: an `argv[0]` (or `argv[0]` + a subcommand) and how the target it
/// starts is named in the rest of the argv.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Launcher {
    /// The names it is started by. A name is matched against `argv[0]`'s
    /// basename, or, holding a `/`, against `argv[0]` whole (`/init`); a
    /// trailing `*` also matches the name followed by a version
    /// (`python*`: `python3`, `python3.11`); a second word must be `argv[1]`
    /// (`bundle exec`). The matched name, without its `*`, is what
    /// [`StartTarget::launcher`] and [`StartTarget::chain`] hold.
    pub names: &'static [&'static str],
    /// Flags whose value is the next argument (`-k` in `gunicorn -k
    /// uvicorn.workers.UvicornWorker`). A `--flag=value` token is one
    /// argument whatever the list says.
    pub value_flags: &'static [&'static str],
    /// Flags that hand the launcher inline code (`-c`, `-e`): refused
    /// [`Refusal::Inline`].
    pub inline_flags: &'static [&'static str],
    /// Words skipped where the target would be (`bun run x.ts`, `tsx watch
    /// x.ts`, `dotnet exec X.dll`).
    pub skip_words: &'static [&'static str],
    /// Positional arguments a wrapper consumes before the command it starts
    /// (`gosu <user> cmd`: 1).
    pub positional: usize,
    /// How its reader starts.
    pub form: TargetForm,
    /// The language the started target is written in, as glia's parsers
    /// name it (`python`, `typescript`); `None` for a wrapper.
    pub lang: Option<&'static str>,
}

impl Launcher {
    const fn new(names: &'static [&'static str], form: TargetForm) -> Self {
        Launcher {
            names,
            value_flags: &[],
            inline_flags: &[],
            skip_words: &[],
            positional: 0,
            form,
            lang: None,
        }
    }

    const fn lang(mut self, lang: &'static str) -> Self {
        self.lang = Some(lang);
        self
    }

    const fn values(mut self, flags: &'static [&'static str]) -> Self {
        self.value_flags = flags;
        self
    }

    const fn inline(mut self, flags: &'static [&'static str]) -> Self {
        self.inline_flags = flags;
        self
    }

    const fn skip(mut self, words: &'static [&'static str]) -> Self {
        self.skip_words = words;
        self
    }

    const fn positional(mut self, n: usize) -> Self {
        self.positional = n;
        self
    }
}

/// The deepest chain of wrappers and shells [`normalise_argv`] reads through
/// (`bash -lc 'dotenv bundle exec sidekiq'` is 3); a fifth hop is refused
/// [`Refusal::Depth`].
pub const MAX_LAUNCH_DEPTH: usize = 4;

/// The launcher table: one row per launcher, read by [`normalise_argv`].
/// These literals live in a `.rs` file, which no needle reads (code_guard).
pub static LAUNCHERS: &[Launcher] = &[
    // Script: the first non-flag argument is a file.
    Launcher::new(&["python*"], TargetForm::Script)
        .lang("python")
        .values(&["-W", "-X", "--check-hash-based-pycs"])
        .inline(&["-c", "-"]),
    Launcher::new(&["node", "nodejs"], TargetForm::Script)
        .lang("typescript")
        .values(&[
            "-r",
            "--require",
            "--import",
            "--loader",
            "--experimental-loader",
            "--env-file",
            "-C",
            "--conditions",
            "--title",
        ])
        .inline(&["-e", "--eval", "-p", "--print"]),
    Launcher::new(&["deno run"], TargetForm::Script)
        .lang("typescript")
        .values(&[
            "-c",
            "--config",
            "--import-map",
            "--lock",
            "--cert",
            "--location",
            "--seed",
        ]),
    Launcher::new(&["bun"], TargetForm::Script)
        .lang("typescript")
        .values(&["--cwd", "-r", "--preload", "--env-file", "-c", "--config"])
        .inline(&["-e", "--eval", "-p", "--print"])
        .skip(&["run"]),
    Launcher::new(&["tsx"], TargetForm::Script)
        .lang("typescript")
        .values(&["--tsconfig", "--import", "-r", "--require"])
        .inline(&["-e", "--eval", "-p", "--print"])
        .skip(&["watch"]),
    Launcher::new(&["ts-node"], TargetForm::Script)
        .lang("typescript")
        .values(&[
            "-P",
            "--project",
            "-r",
            "--require",
            "-C",
            "--compiler",
            "-O",
            "--compiler-options",
            "--cwd",
            "-I",
            "--ignore",
        ])
        .inline(&["-e", "--eval", "-p", "--print"]),
    Launcher::new(&["ruby*"], TargetForm::Script)
        .lang("ruby")
        .values(&["-I", "-r", "-C", "-E"])
        .inline(&["-e"]),
    Launcher::new(&["php*"], TargetForm::Script)
        .lang("php")
        .values(&["-d", "-c", "-z", "-S", "-t"])
        .inline(&["-r", "-a"]),
    Launcher::new(&["lua*", "luajit"], TargetForm::Script)
        .lang("lua")
        .values(&["-l", "-j", "-O"])
        .inline(&["-e", "-"]),
    // Shell: `-c "<str>"`, else a script file.
    Launcher::new(
        &["sh", "bash", "dash", "ash", "zsh", "ksh"],
        TargetForm::Shell,
    )
    .lang("shell")
    .values(&["-o", "-O", "--rcfile", "--init-file"]),
    // ModuleAttr: the first `a.b:c` argument.
    Launcher::new(&["gunicorn"], TargetForm::ModuleAttr)
        .lang("python")
        .values(&[
            "-b",
            "--bind",
            "-c",
            "--config",
            "-w",
            "--workers",
            "-k",
            "--worker-class",
            "-t",
            "--timeout",
            "--threads",
            "--log-level",
            "--access-logfile",
            "--error-logfile",
            "--log-config",
            "-n",
            "--name",
            "--chdir",
            "--pythonpath",
            "-e",
            "--env",
            "-u",
            "--user",
            "-g",
            "--group",
            "-p",
            "--pid",
            "--max-requests",
            "--max-requests-jitter",
            "--graceful-timeout",
            "--keep-alive",
            "--worker-connections",
            "--forwarded-allow-ips",
            "--certfile",
            "--keyfile",
            "--backlog",
            "--paste",
        ]),
    Launcher::new(&["uvicorn"], TargetForm::ModuleAttr)
        .lang("python")
        .values(&[
            "--host",
            "--port",
            "--uds",
            "--fd",
            "--workers",
            "--loop",
            "--http",
            "--ws",
            "--lifespan",
            "--env-file",
            "--log-config",
            "--log-level",
            "--root-path",
            "--app-dir",
            "--forwarded-allow-ips",
            "--ssl-keyfile",
            "--ssl-certfile",
            "--reload-dir",
            "--limit-concurrency",
            "--timeout-keep-alive",
            "--backlog",
            "--limit-max-requests",
            "--header",
        ]),
    Launcher::new(&["hypercorn"], TargetForm::ModuleAttr)
        .lang("python")
        .values(&[
            "-b",
            "--bind",
            "-w",
            "--workers",
            "-k",
            "--worker-class",
            "-c",
            "--config",
            "--access-logfile",
            "--error-logfile",
            "--log-level",
            "--root-path",
            "--certfile",
            "--keyfile",
            "--keep-alive",
            "-g",
            "--group",
            "-u",
            "--user",
            "--insecure-bind",
            "--quic-bind",
        ]),
    Launcher::new(&["daphne"], TargetForm::ModuleAttr)
        .lang("python")
        .values(&[
            "-b",
            "--bind",
            "-p",
            "--port",
            "-u",
            "--unix-socket",
            "--fd",
            "-e",
            "--endpoint",
            "--root-path",
            "--access-log",
            "-t",
            "--http-timeout",
            "-v",
            "--verbosity",
            "--ping-interval",
            "--ping-timeout",
            "--application-close-timeout",
        ]),
    Launcher::new(&["waitress-serve"], TargetForm::ModuleAttr)
        .lang("python")
        .values(&[
            "--host",
            "--port",
            "--listen",
            "--threads",
            "--url-scheme",
            "--url-prefix",
            "--ident",
            "--unix-socket",
            "--channel-timeout",
            "--connection-limit",
        ]),
    // CeleryApp: `-A|--app a.b[:x]`.
    Launcher::new(&["celery"], TargetForm::CeleryApp)
        .lang("python")
        .values(&[
            "-b",
            "--broker",
            "--result-backend",
            "--loader",
            "--config",
            "--workdir",
            "-l",
            "--loglevel",
            "-c",
            "--concurrency",
            "-Q",
            "--queues",
            "-n",
            "--hostname",
            "-P",
            "--pool",
            "-f",
            "--logfile",
            "--pidfile",
            "-s",
            "--schedule",
            "-S",
            "--scheduler",
            "--max-tasks-per-child",
        ]),
    // FlaskApp: `--app|-A x`, else FLASK_APP.
    Launcher::new(&["flask"], TargetForm::FlaskApp)
        .lang("python")
        .values(&[
            "-e",
            "--env-file",
            "-h",
            "--host",
            "-p",
            "--port",
            "--cert",
            "--key",
        ]),
    // PkgScript: a package.json / deno.json script name.
    Launcher::new(&["npm"], TargetForm::PkgScript)
        .lang("typescript")
        .values(&["--prefix", "-w", "--workspace"]),
    Launcher::new(&["yarn"], TargetForm::PkgScript)
        .lang("typescript")
        .values(&["--cwd"]),
    Launcher::new(&["pnpm"], TargetForm::PkgScript)
        .lang("typescript")
        .values(&["--filter", "-F", "-C", "--dir"]),
    Launcher::new(&["deno task"], TargetForm::PkgScript)
        .lang("typescript")
        .values(&["--cwd", "-c", "--config"]),
    // Jar / JavaMain.
    Launcher::new(&["java"], TargetForm::Jar)
        .lang("java")
        .values(&[
            "-cp",
            "-classpath",
            "--class-path",
            "-p",
            "--module-path",
            "--add-opens",
            "--add-exports",
            "--add-modules",
            "--add-reads",
            "--upgrade-module-path",
            "--limit-modules",
            "--patch-module",
        ]),
    // Dll.
    Launcher::new(&["dotnet"], TargetForm::Dll)
        .lang("csharp")
        .values(&[
            "--roll-forward",
            "--fx-version",
            "--additionalprobingpath",
            "--depsfile",
            "--runtimeconfig",
            "--additional-deps",
        ])
        .skip(&["exec"]),
    // Wrapper: strip and re-read.
    Launcher::new(&["tini"], TargetForm::Wrapper).values(&["-p", "-e"]),
    Launcher::new(&["dumb-init"], TargetForm::Wrapper).values(&["-r", "--rewrite"]),
    Launcher::new(&["gosu", "su-exec"], TargetForm::Wrapper).positional(1),
    Launcher::new(&["/init"], TargetForm::Wrapper),
    Launcher::new(&["exec"], TargetForm::Wrapper).values(&["-a"]),
    Launcher::new(&["env"], TargetForm::Wrapper).values(&["-u", "--unset", "-C", "--chdir"]),
    Launcher::new(&["nohup"], TargetForm::Wrapper),
    Launcher::new(&["dotenv"], TargetForm::Wrapper).values(&["-f", "-e", "-c", "-v"]),
    Launcher::new(&["bundle exec"], TargetForm::Wrapper).values(&["--gemfile"]),
    Launcher::new(&["poetry run"], TargetForm::Wrapper).values(&[
        "-C",
        "--directory",
        "-P",
        "--project",
    ]),
    Launcher::new(&["uv run"], TargetForm::Wrapper).values(&[
        "--with",
        "--with-requirements",
        "--with-editable",
        "--project",
        "--directory",
        "--python",
        "-p",
        "--env-file",
        "--extra",
        "--group",
        "--package",
        "--index",
        "--index-url",
    ]),
    Launcher::new(&["pipenv run"], TargetForm::Wrapper),
    Launcher::new(&["npx"], TargetForm::Wrapper)
        .values(&["-p", "--package"])
        .inline(&["-c", "--call"]),
    // Framework: the app layer's to bind.
    Launcher::new(&["rails"], TargetForm::Framework).lang("ruby"),
    Launcher::new(&["puma"], TargetForm::Framework).lang("ruby"),
    Launcher::new(&["sidekiq"], TargetForm::Framework).lang("ruby"),
    Launcher::new(&["manage.py"], TargetForm::Framework).lang("python"),
];

/// What started process an argv names: [`normalise_argv`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StartTarget {
    /// What the target is.
    pub form: TargetForm,
    /// The target as written: a file path, a module path, a script name, a
    /// jar, a main class, a binary. When refused, the text refused (`$APP`,
    /// `"$@"` as `$@`, the shell string), empty for [`Refusal::NoTarget`].
    pub target: String,
    /// The attribute after the target's `:` (`app` of `server:app`, a
    /// factory's name without its call), for [`TargetForm::ModuleAttr`],
    /// [`TargetForm::CeleryApp`] and [`TargetForm::FlaskApp`].
    pub attr: Option<String>,
    /// The arguments after the target (for a celery / flask app, every
    /// argument but the app's flag).
    pub args: Vec<String>,
    /// The launcher row that named the target (its matched name: `python`,
    /// `uvicorn`, `bundle exec`), `None` for a plain [`TargetForm::Binary`].
    pub launcher: Option<&'static str>,
    /// The wrappers and shells read through on the way, outermost first
    /// (`tini -- python -m x`: `["tini"]`; `python -m uvicorn a:b`:
    /// `["python"]`).
    pub chain: Vec<&'static str>,
    /// The language the target is written in (the launcher's `lang`).
    pub lang: Option<&'static str>,
    /// Set when no target could be named.
    pub refused: Option<Refusal>,
}

impl StartTarget {
    /// The reason it was refused, lowercase (`dynamic`), or `None`.
    pub fn refusal(&self) -> Option<&'static str> {
        self.refused.map(Refusal::as_str)
    }
}

/// How an argv starts its target: leading `K=V` assignments, wrappers and
/// `sh -c` strings are read through (at most [`MAX_LAUNCH_DEPTH`] hops) to the
/// launcher that names the target, and that launcher's [`TargetForm`] says how
/// it is named. A `$VAR` / `${..}` target is refused [`Refusal::Dynamic`].
/// Shell strings are read by a line scanner, not a shell: heredocs,
/// functions, conditionals, loops and subshells are refused, never guessed.
pub fn normalise_argv(argv: &[String]) -> StartTarget {
    let mut chain = Vec::new();
    let mut env: Vec<(String, String)> = Vec::new();
    let mut argv: Vec<String> = argv.to_vec();
    loop {
        // Leading `K=V` assignments (a shell's, or `env`'s).
        let assigns = argv.iter().take_while(|t| assignment(t).is_some()).count();
        for t in &argv[..assigns] {
            if let Some((k, v)) = assignment(t) {
                env.push((k.to_string(), v.to_string()));
            }
        }
        argv.drain(..assigns);
        let Some(head) = argv.first() else {
            return refused(
                TargetForm::Binary,
                String::new(),
                Refusal::NoTarget,
                chain,
                None,
                None,
            );
        };
        if is_dynamic(head) {
            let head = head.clone();
            return refused(
                TargetForm::Binary,
                head,
                Refusal::Dynamic,
                chain,
                None,
                None,
            );
        }
        let Some((row, name, words)) = launcher_for(&argv) else {
            let target = argv.remove(0);
            return done(TargetForm::Binary, target, None, argv, None, chain, None);
        };
        let rest = argv[words..].to_vec();
        let step = match row.form {
            TargetForm::Wrapper => read_wrapper(row, name, &rest),
            TargetForm::Shell => read_shell(row, name, &rest),
            TargetForm::Script => read_script(row, name, &rest),
            TargetForm::ModuleAttr => Step::Done(read_module_attr(row, name, &rest)),
            TargetForm::CeleryApp => Step::Done(read_celery(row, name, &rest)),
            TargetForm::FlaskApp => Step::Done(read_flask(row, name, &rest, &env)),
            TargetForm::PkgScript => read_pkg_script(row, name, words, &rest),
            TargetForm::Jar | TargetForm::JavaMain => Step::Done(read_java(row, name, &rest)),
            TargetForm::Dll => Step::Done(read_dotnet(row, name, &rest)),
            TargetForm::Framework => {
                let target = argv[0].clone();
                Step::Done(partial(
                    TargetForm::Framework,
                    target,
                    None,
                    rest,
                    row,
                    name,
                ))
            }
            // A launcher row never declares these two forms; read as Binary.
            TargetForm::PyModule | TargetForm::Binary => {
                let target = argv[0].clone();
                Step::Done(partial(TargetForm::Binary, target, None, rest, row, name))
            }
        };
        match step {
            Step::Done(mut t) => {
                t.chain = chain;
                return t;
            }
            Step::Again(next) => {
                if chain.len() >= MAX_LAUNCH_DEPTH {
                    let target = next.first().cloned().unwrap_or_default();
                    return refused(
                        row.form,
                        target,
                        Refusal::Depth,
                        chain,
                        Some(name),
                        row.lang,
                    );
                }
                chain.push(name);
                argv = next;
            }
        }
    }
}

/// One reader's answer: the target, or the argv to re-read.
enum Step {
    Done(StartTarget),
    Again(Vec<String>),
}

fn done(
    form: TargetForm,
    target: String,
    attr: Option<String>,
    args: Vec<String>,
    launcher: Option<&'static str>,
    chain: Vec<&'static str>,
    lang: Option<&'static str>,
) -> StartTarget {
    let refused = is_dynamic(&target).then_some(Refusal::Dynamic);
    StartTarget {
        form,
        target,
        attr,
        args,
        launcher,
        chain,
        lang,
        refused,
    }
}

fn refused(
    form: TargetForm,
    target: String,
    why: Refusal,
    chain: Vec<&'static str>,
    launcher: Option<&'static str>,
    lang: Option<&'static str>,
) -> StartTarget {
    StartTarget {
        form,
        target,
        attr: None,
        args: Vec::new(),
        launcher,
        chain,
        lang,
        refused: Some(why),
    }
}

/// A reader's target, its chain filled in by [`normalise_argv`].
fn partial(
    form: TargetForm,
    target: String,
    attr: Option<String>,
    args: Vec<String>,
    row: &Launcher,
    name: &'static str,
) -> StartTarget {
    done(form, target, attr, args, Some(name), Vec::new(), row.lang)
}

fn refuse(
    form: TargetForm,
    target: &str,
    why: Refusal,
    row: &Launcher,
    name: &'static str,
) -> StartTarget {
    refused(
        form,
        target.to_string(),
        why,
        Vec::new(),
        Some(name),
        row.lang,
    )
}

/// `K=V` as a shell assignment word.
fn assignment(t: &str) -> Option<(&str, &str)> {
    let (k, v) = t.split_once('=')?;
    let mut bytes = k.bytes();
    let first = bytes.next()?;
    ((first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_'))
    .then_some((k, v))
}

/// A runtime value: a parameter expansion or a command substitution.
fn is_dynamic(t: &str) -> bool {
    t.contains('$') || t.contains('`')
}

/// The launcher row an argv starts with: its row, the matched name (without
/// a trailing `*`) and how many argv words it took. A two-word name wins over
/// a one-word one; table order breaks a tie.
fn launcher_for(argv: &[String]) -> Option<(&'static Launcher, &'static str, usize)> {
    let head = argv.first()?;
    let base = head.rsplit('/').next().unwrap_or(head);
    let mut best: Option<(&'static Launcher, &'static str, usize)> = None;
    for row in LAUNCHERS {
        for &pattern in row.names {
            let mut words = pattern.split(' ');
            let first = words.next().unwrap_or(pattern);
            let against = if first.contains('/') {
                head.as_str()
            } else {
                base
            };
            if !name_matches(first, against) {
                continue;
            }
            let more: Vec<&str> = words.collect();
            let n = 1 + more.len();
            if argv.len() < n || more.iter().zip(&argv[1..]).any(|(w, a)| w != a) {
                continue;
            }
            if best.is_none_or(|(_, _, m)| n > m) {
                best = Some((row, pattern.strip_suffix('*').unwrap_or(pattern), n));
            }
        }
    }
    best
}

/// `python*` matches `python`, `python3`, `python3.11`; any other name only
/// itself.
fn name_matches(pattern: &str, word: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(stem) => word
            .strip_prefix(stem)
            .is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit() || b == b'.')),
        None => pattern == word,
    }
}

/// Where a launcher's options end.
enum Scan {
    /// The first positional argument's index.
    Positional(usize),
    /// One of the caller's stop flags, at its index.
    Stop(usize),
    /// An inline-code flag.
    Inline,
    /// No positional argument.
    End,
}

/// Skip `row`'s options in `args`: `--` ends them, a value flag takes the
/// next argument, any other `-x` / `--x=v` token is one argument; `stops` are
/// flags the caller reads itself.
fn scan(args: &[String], row: &Launcher, stops: &[&str]) -> Scan {
    let mut i = 0;
    while i < args.len() {
        let t = args[i].as_str();
        if t == "--" {
            return if i + 1 < args.len() {
                Scan::Positional(i + 1)
            } else {
                Scan::End
            };
        }
        if row.inline_flags.contains(&t) {
            return Scan::Inline;
        }
        if stops.contains(&t) {
            return Scan::Stop(i);
        }
        if t.len() > 1 && t.starts_with('-') {
            i += if row.value_flags.contains(&t) { 2 } else { 1 };
            continue;
        }
        if row.skip_words.contains(&t) {
            i += 1;
            continue;
        }
        return Scan::Positional(i);
    }
    Scan::End
}

fn read_wrapper(row: &Launcher, name: &'static str, rest: &[String]) -> Step {
    match scan(rest, row, &[]) {
        Scan::Positional(i) if i + row.positional < rest.len() => {
            Step::Again(rest[i + row.positional..].to_vec())
        }
        Scan::Inline => Step::Done(refuse(TargetForm::Wrapper, "", Refusal::Inline, row, name)),
        _ => Step::Done(refuse(
            TargetForm::Wrapper,
            "",
            Refusal::NoTarget,
            row,
            name,
        )),
    }
}

fn read_shell(row: &Launcher, name: &'static str, rest: &[String]) -> Step {
    let mut i = 0;
    let mut command = false;
    while i < rest.len() {
        let t = rest[i].as_str();
        if t == "--" {
            i += 1;
            break;
        }
        if !(t.len() > 1 && (t.starts_with('-') || t.starts_with('+'))) {
            break;
        }
        if row.value_flags.contains(&t) {
            i += 2;
            continue;
        }
        if !t.starts_with("--") && t[1..].contains('c') {
            command = true;
        }
        i += 1;
    }
    let Some(first) = rest.get(i) else {
        return Step::Done(refuse(TargetForm::Shell, "", Refusal::NoTarget, row, name));
    };
    if !command {
        let target = first.clone();
        return Step::Done(partial(
            TargetForm::Script,
            target,
            None,
            rest[i + 1..].to_vec(),
            row,
            name,
        ));
    }
    match split_shell(first) {
        Some(words) if !words.is_empty() => Step::Again(words),
        Some(_) => Step::Done(refuse(
            TargetForm::Shell,
            first,
            Refusal::NoTarget,
            row,
            name,
        )),
        None => Step::Done(refuse(
            TargetForm::Shell,
            first,
            Refusal::Dynamic,
            row,
            name,
        )),
    }
}

fn read_script(row: &Launcher, name: &'static str, rest: &[String]) -> Step {
    let python = row.lang == Some("python");
    let stops: &[&str] = if python { &["-m"] } else { &[] };
    let i = match scan(rest, row, stops) {
        Scan::Positional(i) => i,
        Scan::Stop(i) => {
            // `python -m a.b`: a module, or a launcher run as one
            // (`python -m uvicorn a:b`).
            let Some(module) = rest.get(i + 1) else {
                return Step::Done(refuse(
                    TargetForm::PyModule,
                    "",
                    Refusal::NoTarget,
                    row,
                    name,
                ));
            };
            let again: Vec<String> = rest[i + 1..].to_vec();
            if let Some((next, _, _)) = launcher_for(&again)
                && next.lang == Some("python")
                && next.form != TargetForm::Script
            {
                return Step::Again(again);
            }
            let args = rest[i + 2..].to_vec();
            return Step::Done(partial(
                TargetForm::PyModule,
                module.clone(),
                None,
                args,
                row,
                name,
            ));
        }
        Scan::Inline => {
            return Step::Done(refuse(TargetForm::Script, "", Refusal::Inline, row, name));
        }
        Scan::End => {
            return Step::Done(refuse(TargetForm::Script, "", Refusal::NoTarget, row, name));
        }
    };
    let file = &rest[i];
    // A framework runner started through its interpreter (`python
    // manage.py runserver`, `ruby bin/rails server`).
    let again: Vec<String> = rest[i..].to_vec();
    if let Some((next, _, _)) = launcher_for(&again)
        && next.form == TargetForm::Framework
    {
        return Step::Again(again);
    }
    // `bun x` with no extension or path is a package script.
    if name == "bun" && !file.contains('.') && !file.contains('/') {
        return Step::Done(partial(
            TargetForm::PkgScript,
            file.clone(),
            None,
            rest[i + 1..].to_vec(),
            row,
            name,
        ));
    }
    Step::Done(partial(
        TargetForm::Script,
        file.clone(),
        None,
        rest[i + 1..].to_vec(),
        row,
        name,
    ))
}

/// `a.b:c` (or `a.b:c()`): the module and the attribute's name.
fn module_attr(t: &str) -> Option<(&str, &str)> {
    let (module, attr) = t.split_once(':')?;
    let attr = attr.split('(').next().unwrap_or(attr);
    (is_dotted_identifier(module) && is_dotted_identifier(attr)).then_some((module, attr))
}

fn is_dotted_identifier(t: &str) -> bool {
    !t.is_empty()
        && t.split('.').all(|seg| {
            let mut b = seg.bytes();
            b.next()
                .is_some_and(|f| f.is_ascii_alphabetic() || f == b'_')
                && b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
}

/// The positional arguments of `args`, skipping `row`'s options.
fn positionals<'a>(args: &'a [String], row: &Launcher) -> Vec<(usize, &'a str)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match scan(&args[i..], row, &[]) {
            Scan::Positional(j) => {
                out.push((i + j, args[i + j].as_str()));
                i += j + 1;
            }
            _ => break,
        }
    }
    out
}

fn read_module_attr(row: &Launcher, name: &'static str, rest: &[String]) -> StartTarget {
    let pos = positionals(rest, row);
    let without = |i: usize| -> Vec<String> {
        rest.iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, t)| t.clone())
            .collect()
    };
    if let Some((i, (module, attr))) = pos
        .iter()
        .find_map(|(i, t)| module_attr(t).map(|m| (*i, m)))
    {
        let (module, attr) = (module.to_string(), attr.to_string());
        return partial(
            TargetForm::ModuleAttr,
            module,
            Some(attr),
            without(i),
            row,
            name,
        );
    }
    match pos.first() {
        Some((_, t)) if is_dynamic(t) => {
            refuse(TargetForm::ModuleAttr, t, Refusal::Dynamic, row, name)
        }
        Some((i, t)) if is_dotted_identifier(t) => partial(
            TargetForm::ModuleAttr,
            t.to_string(),
            None,
            without(*i),
            row,
            name,
        ),
        _ => refuse(TargetForm::ModuleAttr, "", Refusal::NoTarget, row, name),
    }
}

/// The value of `short` / `long` in `args` (`-A x`, `--app x`, `--app=x`),
/// and `args` without it.
fn take_flag(args: &[String], short: &str, long: &str) -> Option<(String, Vec<String>)> {
    let eq = format!("{long}=");
    for (i, t) in args.iter().enumerate() {
        if t == short || t == long {
            let value = args.get(i + 1)?.clone();
            let mut rest = args[..i].to_vec();
            rest.extend_from_slice(&args[i + 2..]);
            return Some((value, rest));
        }
        if let Some(v) = t.strip_prefix(&eq) {
            let mut rest = args[..i].to_vec();
            rest.extend_from_slice(&args[i + 1..]);
            return Some((v.to_string(), rest));
        }
    }
    None
}

/// `a.b:x` -> (`a.b`, `x`); `a.b` -> (`a.b`, None).
fn split_app(value: &str) -> (String, Option<String>) {
    match value.split_once(':') {
        Some((m, a)) => (
            m.to_string(),
            Some(a.split('(').next().unwrap_or(a).to_string()),
        ),
        None => (value.to_string(), None),
    }
}

fn read_celery(row: &Launcher, name: &'static str, rest: &[String]) -> StartTarget {
    match take_flag(rest, "-A", "--app") {
        Some((app, args)) => {
            let (module, attr) = split_app(&app);
            partial(TargetForm::CeleryApp, module, attr, args, row, name)
        }
        None => refuse(TargetForm::CeleryApp, "", Refusal::NoTarget, row, name),
    }
}

fn read_flask(
    row: &Launcher,
    name: &'static str,
    rest: &[String],
    env: &[(String, String)],
) -> StartTarget {
    let (app, args) = match take_flag(rest, "-A", "--app") {
        Some(found) => found,
        None => match env.iter().rev().find(|(k, _)| k == "FLASK_APP") {
            Some((_, v)) => (v.clone(), rest.to_vec()),
            None => return refuse(TargetForm::FlaskApp, "", Refusal::NoTarget, row, name),
        },
    };
    let (module, attr) = split_app(&app);
    partial(TargetForm::FlaskApp, module, attr, args, row, name)
}

fn read_pkg_script(row: &Launcher, name: &'static str, words: usize, rest: &[String]) -> Step {
    let Scan::Positional(i) = scan(rest, row, &[]) else {
        return Step::Done(refuse(
            TargetForm::PkgScript,
            "",
            Refusal::NoTarget,
            row,
            name,
        ));
    };
    let script = |j: usize| -> Step {
        let Some(tail) = rest.get(j..) else {
            return Step::Done(refuse(
                TargetForm::PkgScript,
                "",
                Refusal::NoTarget,
                row,
                name,
            ));
        };
        match scan(tail, row, &[]) {
            Scan::Positional(k) => {
                let at = j + k;
                let args = rest[at + 1..]
                    .iter()
                    .filter(|t| *t != "--")
                    .cloned()
                    .collect();
                Step::Done(partial(
                    TargetForm::PkgScript,
                    rest[at].clone(),
                    None,
                    args,
                    row,
                    name,
                ))
            }
            _ => Step::Done(refuse(
                TargetForm::PkgScript,
                "",
                Refusal::NoTarget,
                row,
                name,
            )),
        }
    };
    // `deno task x`: the subcommand is the name's second word.
    if words > 1 {
        return script(i);
    }
    match rest[i].as_str() {
        "run" | "run-script" => script(i + 1),
        // `npm exec` / `pnpm exec` / `yarn exec`: a wrapper.
        "exec" | "x" => match scan(&rest[i + 1..], row, &[]) {
            Scan::Positional(k) => Step::Again(rest[i + 1 + k..].to_vec()),
            _ => Step::Done(refuse(
                TargetForm::Wrapper,
                "",
                Refusal::NoTarget,
                row,
                name,
            )),
        },
        // `yarn workspace <name> <script>`.
        "workspace" if name == "yarn" => script(i + 2),
        // npm runs only these lifecycle scripts without `run`.
        "start" | "test" | "stop" | "restart" if name == "npm" => Step::Done(partial(
            TargetForm::PkgScript,
            rest[i].clone(),
            None,
            rest[i + 1..].to_vec(),
            row,
            name,
        )),
        _ if name == "npm" => Step::Done(partial(
            TargetForm::Binary,
            name.to_string(),
            None,
            rest.to_vec(),
            row,
            name,
        )),
        _ => {
            let args = rest[i + 1..]
                .iter()
                .filter(|t| *t != "--")
                .cloned()
                .collect();
            Step::Done(partial(
                TargetForm::PkgScript,
                rest[i].clone(),
                None,
                args,
                row,
                name,
            ))
        }
    }
}

fn read_java(row: &Launcher, name: &'static str, rest: &[String]) -> StartTarget {
    match scan(rest, row, &["-jar", "-m", "--module"]) {
        Scan::Stop(i) => {
            let form = if rest[i] == "-jar" {
                TargetForm::Jar
            } else {
                TargetForm::JavaMain
            };
            match rest.get(i + 1) {
                Some(t) => partial(form, t.clone(), None, rest[i + 2..].to_vec(), row, name),
                None => refuse(form, "", Refusal::NoTarget, row, name),
            }
        }
        Scan::Positional(i) => partial(
            TargetForm::JavaMain,
            rest[i].clone(),
            None,
            rest[i + 1..].to_vec(),
            row,
            name,
        ),
        _ => refuse(TargetForm::JavaMain, "", Refusal::NoTarget, row, name),
    }
}

fn read_dotnet(row: &Launcher, name: &'static str, rest: &[String]) -> StartTarget {
    let Scan::Positional(i) = scan(rest, row, &[]) else {
        return refuse(TargetForm::Dll, "", Refusal::NoTarget, row, name);
    };
    let t = &rest[i];
    if t.to_ascii_lowercase().ends_with(".dll") {
        return partial(
            TargetForm::Dll,
            t.clone(),
            None,
            rest[i + 1..].to_vec(),
            row,
            name,
        );
    }
    if t == "run" {
        let after = &rest[i + 1..];
        return match take_flag(after, "-p", "--project") {
            Some((project, args)) => partial(TargetForm::Dll, project, None, args, row, name),
            None => refuse(TargetForm::Dll, "", Refusal::NoTarget, row, name),
        };
    }
    partial(
        TargetForm::Binary,
        name.to_string(),
        None,
        rest.to_vec(),
        row,
        name,
    )
}

/// Shell words that open or close a compound command or a function: a line
/// scanner cannot say which command such a string runs last.
const SHELL_CONTROL: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done",
    "function", "select", "{", "}", "[[",
];

/// One simple command while splitting.
#[derive(Default)]
struct ShellCommand {
    words: Vec<String>,
    /// A command substitution sits in it.
    dynamic: bool,
}

/// Split a `sh -c` string with shell-word rules (quotes, backslashes,
/// comments, redirections dropped) and return the command it starts: the
/// first command of the last pipeline of its `;` / `&&` / `||` / `&` /
/// newline chain. `None` when the string holds what a line scanner does not
/// read (a heredoc, a subshell or function, a compound command, a command
/// substitution in the chosen command, an unterminated quote).
fn split_shell(s: &str) -> Option<Vec<String>> {
    // Pipelines in order; each a list of commands.
    let mut pipelines: Vec<Vec<ShellCommand>> = vec![vec![ShellCommand::default()]];
    let mut word = String::new();
    let mut in_word = false;
    let mut redirect_target = false;
    let mut chars = s.chars().peekable();

    fn current(p: &mut [Vec<ShellCommand>]) -> Option<&mut ShellCommand> {
        p.last_mut()?.last_mut()
    }
    fn end_word(
        p: &mut [Vec<ShellCommand>],
        word: &mut String,
        in_word: &mut bool,
        redirect_target: &mut bool,
    ) {
        if *in_word {
            if *redirect_target {
                *redirect_target = false;
            } else if let Some(c) = current(p) {
                c.words.push(std::mem::take(word));
            }
        }
        word.clear();
        *in_word = false;
    }

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\r' => end_word(
                &mut pipelines,
                &mut word,
                &mut in_word,
                &mut redirect_target,
            ),
            '\n' | ';' => {
                if c == ';' && chars.peek() == Some(&';') {
                    return None;
                }
                end_word(
                    &mut pipelines,
                    &mut word,
                    &mut in_word,
                    &mut redirect_target,
                );
                pipelines.push(vec![ShellCommand::default()]);
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(n) => {
                    word.push(n);
                    in_word = true;
                }
                None => {}
            },
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        n => word.push(n),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            '\n' => {}
                            n @ ('"' | '\\' | '$' | '`') => word.push(n),
                            n => {
                                word.push('\\');
                                word.push(n);
                            }
                        },
                        '`' => {
                            current(&mut pipelines)?.dynamic = true;
                            word.push('`');
                        }
                        '$' if chars.peek() == Some(&'(') => {
                            current(&mut pipelines)?.dynamic = true;
                            word.push('$');
                        }
                        n => word.push(n),
                    }
                }
            }
            '`' => {
                current(&mut pipelines)?.dynamic = true;
                word.push('`');
                in_word = true;
            }
            '$' if matches!(chars.peek(), Some('(') | Some('{')) => {
                // A substitution or a parameter expansion, kept whole.
                let open = chars.next()?;
                let close = if open == '(' { ')' } else { '}' };
                if open == '(' {
                    current(&mut pipelines)?.dynamic = true;
                }
                word.push('$');
                word.push(open);
                in_word = true;
                let mut depth = 1usize;
                while depth > 0 {
                    let n = chars.next()?;
                    if n == open {
                        depth += 1;
                    } else if n == close {
                        depth -= 1;
                    }
                    word.push(n);
                }
            }
            '#' if !in_word => {
                while chars.peek().is_some_and(|&n| n != '\n') {
                    chars.next();
                }
            }
            '&' => {
                if chars.peek() == Some(&'>') {
                    // `&>file` / `&>>file`.
                    end_word(
                        &mut pipelines,
                        &mut word,
                        &mut in_word,
                        &mut redirect_target,
                    );
                    chars.next();
                    if chars.peek() == Some(&'>') {
                        chars.next();
                    }
                    redirect_target = true;
                } else {
                    if chars.peek() == Some(&'&') {
                        chars.next();
                    }
                    end_word(
                        &mut pipelines,
                        &mut word,
                        &mut in_word,
                        &mut redirect_target,
                    );
                    pipelines.push(vec![ShellCommand::default()]);
                }
            }
            '|' => {
                let or = chars.peek() == Some(&'|');
                if or || chars.peek() == Some(&'&') {
                    chars.next();
                }
                end_word(
                    &mut pipelines,
                    &mut word,
                    &mut in_word,
                    &mut redirect_target,
                );
                if or {
                    pipelines.push(vec![ShellCommand::default()]);
                } else if let Some(p) = pipelines.last_mut() {
                    p.push(ShellCommand::default());
                }
            }
            '>' | '<' => {
                if c == '<' && chars.peek() == Some(&'<') {
                    return None;
                }
                // `2>` / `2>&1`: the fd digits are the redirection's.
                if in_word && !word.is_empty() && word.bytes().all(|b| b.is_ascii_digit()) {
                    word.clear();
                    in_word = false;
                } else {
                    end_word(
                        &mut pipelines,
                        &mut word,
                        &mut in_word,
                        &mut redirect_target,
                    );
                }
                if matches!(chars.peek(), Some('>') | Some('&') | Some('|')) {
                    chars.next();
                }
                redirect_target = true;
            }
            '(' | ')' => return None,
            n => {
                word.push(n);
                in_word = true;
            }
        }
    }
    end_word(
        &mut pipelines,
        &mut word,
        &mut in_word,
        &mut redirect_target,
    );

    let heads = pipelines.iter().flatten().filter_map(|c| c.words.first());
    for head in heads {
        if SHELL_CONTROL.contains(&head.as_str()) {
            return None;
        }
    }
    let Some(last) = pipelines
        .into_iter()
        .rev()
        .find(|p| p.iter().any(|c| !c.words.is_empty()))
    else {
        return Some(Vec::new());
    };
    let first = last.into_iter().find(|c| !c.words.is_empty())?;
    (!first.dynamic).then_some(first.words)
}

/// Every problem in a launcher table: an empty or repeated name, a `*` not at
/// a name's end, a name of more than two words, a flag not starting `-` or
/// both a value and an inline flag, a positional count on a non-wrapper, and a
/// language on a wrapper. Empty = valid.
pub fn validate_launcher_rows(rows: &[Launcher]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for row in rows {
        let at = format!("launcher {:?}", row.names.first().copied().unwrap_or(""));
        if row.names.is_empty() {
            problems.push(format!("{at}: no names"));
        }
        for &n in row.names {
            let words: Vec<&str> = n.split(' ').collect();
            if n.is_empty() || words.len() > 2 || words.iter().any(|w| w.is_empty()) {
                problems.push(format!("{at}: name {n:?} must be one or two words"));
            }
            if n.trim_end_matches('*').contains('*') || (n.ends_with('*') && words.len() > 1) {
                problems.push(format!("{at}: name {n:?} may end in `*` only as one word"));
            }
            if seen.contains(&n) {
                problems.push(format!("{at}: duplicate name {n:?}"));
            }
            seen.push(n);
        }
        for f in row.value_flags.iter().chain(row.inline_flags) {
            if !f.starts_with('-') {
                problems.push(format!("{at}: flag {f:?} must start with `-`"));
            }
        }
        if row.value_flags.iter().any(|f| row.inline_flags.contains(f)) {
            problems.push(format!("{at}: a flag is both a value and an inline flag"));
        }
        if row.positional > 0 && row.form != TargetForm::Wrapper {
            problems.push(format!("{at}: only a wrapper consumes positionals"));
        }
        if row.form == TargetForm::Wrapper && row.lang.is_some() {
            problems.push(format!("{at}: a wrapper has no language"));
        }
        if matches!(row.form, TargetForm::PyModule | TargetForm::Binary) {
            problems.push(format!(
                "{at}: {} is a reader's answer, not a row's form",
                row.form.as_str()
            ));
        }
    }
    problems
}

/// [`validate_launcher_rows`] over [`LAUNCHERS`].
pub fn validate_launchers() -> Vec<String> {
    validate_launcher_rows(LAUNCHERS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_valid() {
        assert_eq!(validate(), Vec::<String>::new());
        // Maki first: PR #3's seed.
        let names: Vec<&str> = HOSTS.iter().map(|h| h.name).collect();
        assert_eq!(names, ["maki", "neovim", "wezterm"]);
    }

    #[test]
    fn validate_names_each_problem() {
        let bad = [
            HostRow {
                name: "env",
                lang: "lua",
                exts: &[".lua"],
                gate: Gate::Global("x"),
                loads: &[""],
                registrations: &[
                    Registration {
                        callee: Callee::Exact("y.f"),
                        pattern: Pattern::Command,
                        shape: Shape::Positional {
                            name_arg: 1,
                            handler_arg: 1,
                            handler_table: None,
                        },
                    },
                    Registration {
                        callee: Callee::Prefix {
                            prefix: "x.reg_",
                            mint: &[],
                        },
                        pattern: Pattern::Event,
                        shape: Shape::Table {
                            name_key: "name",
                            handler_keys: &["name"],
                        },
                    },
                ],
                callbacks: &[Callback {
                    prefix: "",
                    names: &[],
                }],
                config: &[
                    ConfigRule {
                        flavour: ConfigFlavour::Assign,
                        prefixes: &["x.o", "y.o"],
                        namespaced: &["x.g"],
                    },
                    ConfigRule {
                        flavour: ConfigFlavour::Assign,
                        prefixes: &[],
                        namespaced: &[],
                    },
                ],
            },
            HOSTS[0],
            HOSTS[0],
        ];
        let problems = validate_rows(&bad);
        let has = |s: &str| problems.iter().any(|p| p.contains(s));
        assert!(has("\"env\": name is reserved"), "{problems:?}");
        assert!(has("exts without dots"));
        assert!(has("not rooted at `x.`"));
        assert!(has("distinct 1-based arguments"));
        assert!(has("needs identifier mint suffixes"));
        assert!(has("distinct handler keys"));
        assert!(has("empty load glob"));
        assert!(has("callback rule needs a prefix"));
        assert!(has("Assign config rule needs prefixes"));
        assert!(has("Assign config flavour declared twice"));
        assert!(has("\"maki\": duplicate name"));
        assert!(has("config prefix \"y.o\" is not rooted at `x.`"));
        assert!(has("namespaced prefix \"x.g\" is not one of"));
        assert_eq!(problems.len(), 13, "{problems:?}");
    }

    #[test]
    fn config_for_reads_one_key_under_the_longest_prefix() {
        let m = config_for("lua", "vim.opt.number").expect("neovim option");
        assert_eq!((m.host.name, m.key.as_str()), ("neovim", "number"));
        assert_eq!(m.flavour, ConfigFlavour::Assign);
        // `vim.o.` is no prefix of `vim.opt_local.x`; the longer one wins.
        let m = config_for("lua", "vim.opt_local.spell").expect("opt_local");
        assert_eq!(m.key, "spell");
        // vim.g keys are variables: their own namespace.
        let m = config_for("lua", "vim.g.mapleader").expect("vim.g");
        assert_eq!(m.key, "g.mapleader");
        assert_eq!(
            host_config_qname(m.host.name, &m.key),
            "config:neovim:g.mapleader"
        );
        // A builder's local reads through its call.
        let m = config_for("lua", "wezterm.config_builder().font_size").expect("builder");
        assert_eq!((m.host.name, m.key.as_str()), ("wezterm", "font_size"));
        assert_eq!(m.flavour, ConfigFlavour::Builder);
        for none in [
            "vim.opt",
            "vim.opt.x.y",
            "vim.api.nvim_set_option",
            "wezterm.config_builder.font_size",
            "wezterm.font_size",
        ] {
            assert!(config_for("lua", none).is_none(), "{none}");
        }
        assert!(config_for("python", "vim.opt.number").is_none());
    }

    #[test]
    fn builders_requires_and_host_config_qnames() {
        assert_eq!(
            config_builder_for("lua", "wezterm.config_builder").map(|h| h.name),
            Some("wezterm")
        );
        assert!(config_builder_for("lua", "wezterm.font").is_none());
        assert_eq!(
            host_for_require("lua", "wezterm").map(|h| h.name),
            Some("wezterm")
        );
        // Neovim is a global, never a require.
        assert!(host_for_require("lua", "vim").is_none());
        assert!(!is_host_global("lua", "wezterm"));
        assert_eq!(
            returned_config_host("lua", ["lapis", "wezterm"].into_iter()).map(|h| h.name),
            Some("wezterm")
        );
        assert!(returned_config_host("lua", ["lapis"].into_iter()).is_none());
        assert_eq!(
            split_host_config_qname("config:wezterm:font_size"),
            Some(("wezterm", "font_size"))
        );
        assert_eq!(
            split_host_config_qname("config:neovim:g.mapleader"),
            Some(("neovim", "g.mapleader"))
        );
        assert!(split_host_config_qname("config:env:DATABASE_URL").is_none());
        assert!(split_host_config_qname("host_cmd:neovim:Greet").is_none());
    }

    #[test]
    fn push_config_key_mints_once_and_edges_once_per_side() {
        let repo = RepoId(7);
        let module = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "init");
        let (mut nodes, mut nav, mut refs, mut edges) =
            (Vec::new(), CodeNav::default(), Vec::new(), Vec::new());
        let found = config_for("lua", "vim.o.number").expect("option");
        let site = HostSite::new(repo, module, "neovim", 3, Vec::new());
        let mut sink = HostSink::new(&mut nodes, &mut nav, &mut refs);
        let a = push_config_key(
            &site,
            &found,
            ConfigSide::Define,
            module,
            &mut sink,
            &mut edges,
        );
        let b = push_config_key(
            &site,
            &found,
            ConfigSide::Define,
            module,
            &mut sink,
            &mut edges,
        );
        let c = push_config_key(
            &site,
            &found,
            ConfigSide::Read,
            module,
            &mut sink,
            &mut edges,
        );
        assert!(a == b && b == c);
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nav.qname_by_id.get(&a).map(String::as_str),
            Some("config:neovim:number")
        );
        let cats: Vec<_> = edges.iter().map(|e| e.category).collect();
        assert_eq!(
            cats,
            [edge_category::DEFINES_CONFIG, edge_category::READS_CONFIG]
        );
        let ev = crate::evidence::Evidence::of(&edges[0]).expect("evidence");
        assert_eq!(ev.emitter, "parser:lua");
        assert_eq!(ev.rule.as_deref(), Some("host_config_assign"));
        assert_eq!(ev.line, Some(3));
        assert!(refs.is_empty());
    }

    #[test]
    fn registration_for_takes_the_longest_callee() {
        let m = registration_for("lua", "vim.api.nvim_create_user_command").expect("neovim");
        assert_eq!(m.host.name, "neovim");
        assert!(m.mints);
        assert_eq!(m.suffix, "");
        // An exact callee never matches a longer name.
        assert!(registration_for("lua", "vim.api.nvim_create_user_command2").is_none());
        // Keymaps are no commands.
        assert!(registration_for("lua", "vim.keymap.set").is_none());
        assert!(registration_for("lua", "vim.api.nvim_set_keymap").is_none());

        let tool = registration_for("lua", "maki.api.register_tool").expect("maki");
        assert_eq!(
            (tool.host.name, tool.suffix, tool.mints),
            ("maki", "tool", true)
        );
        let cmd = registration_for("lua", "maki.api.register_command").expect("maki");
        assert!(cmd.mints);
        let aux = registration_for("lua", "maki.api.register_hook").expect("maki aux");
        assert_eq!((aux.suffix, aux.mints), ("hook", false));
        // A prefix needs an identifier tail.
        assert!(registration_for("lua", "maki.api.register_").is_none());
        assert!(registration_for("lua", "maki.api.register_tool.x").is_none());
        // Another language's lookup sees none of Lua's rows.
        assert!(registration_for("python", "vim.api.nvim_create_user_command").is_none());
    }

    #[test]
    fn qnames_and_handler_keys() {
        assert_eq!(
            host_command_qname("neovim", "Greet"),
            "host_cmd:neovim:Greet"
        );
        assert_eq!(
            host_event_qname("neovim", "BufWritePost"),
            "host_event:neovim:BufWritePost"
        );
        assert_eq!(split_host_qname("host_cmd:maki:a:b"), Some(("maki", "a:b")));
        assert_eq!(split_host_qname("cli:x"), None);
        assert_eq!(Pattern::Command.handler_key("t", None), "command t");
        assert_eq!(
            Pattern::Command.handler_key("t", Some("restore")),
            "command t restore"
        );
        assert_eq!(Pattern::Event.handler_key("E", None), "on E");
        assert!(is_host_global("lua", "vim"));
        assert!(is_host_global("lua", "maki"));
        assert!(!is_host_global("lua", "awful"));
    }

    #[test]
    fn push_command_mints_once_and_refs_each_handler() {
        let module = NodeId::from_parts(GRAPH_TYPE, RepoId(3), node_kind::MODULE, "plugin::greet");
        let (mut nodes, mut nav, mut refs) = (Vec::new(), CodeNav::default(), Vec::new());
        let site = HostSite::new(RepoId(3), module, "neovim", 5, Vec::new());
        let mut sink = HostSink::new(&mut nodes, &mut nav, &mut refs);
        let a = push_command(
            &site,
            "Greet",
            &[CallQualifier::Bare("greet".into())],
            &mut sink,
        );
        let b = push_command(
            &site,
            "Greet",
            &[CallQualifier::Bare("other".into())],
            &mut sink,
        );
        let e = push_event(&site, "BufWritePost", &[], &mut sink);
        assert_eq!(a, b);
        assert_ne!(a, e);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nav.qname_by_id[&a], "host_cmd:neovim:Greet");
        assert_eq!(nav.name_by_id[&a], "Greet");
        assert_eq!(nav.kind_by_id[&a], node_kind::CLI_COMMAND);
        assert_eq!(nav.kind_by_id[&e], node_kind::EVENT_HANDLER);
        assert!(!nav.parent_of.contains_key(&a));
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().all(|r| r.from == a
            && r.from_module == module
            && r.category == edge_category::HANDLED_BY
            && r.line == 5));
    }

    #[test]
    fn the_marker() {
        let mut t = HostTally::default();
        assert_eq!(t.marker("lua", "a.lua"), None);
        t.registered(Pattern::Command);
        t.registered(Pattern::Command);
        t.minted = 1;
        t.hit("neovim");
        t.hit("neovim");
        assert_eq!(
            t.marker("lua", "plugin/greet.lua").as_deref(),
            Some(
                "[host] lang=lua commands=2 events=0 callbacks=0 config=0 minted=1 aux=0 unread=0 hosts=neovim:2 file=plugin/greet.lua"
            )
        );
    }
}

/// The process layer's tests (GH.14): `cargo test -p glia-code-domain
/// hosts::launchers`.
#[cfg(test)]
mod launchers {
    use super::*;

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|t| t.to_string()).collect()
    }

    fn norm(s: &[&str]) -> StartTarget {
        normalise_argv(&argv(s))
    }

    /// (form, target, attr, launcher, chain, refusal) of one argv.
    type Seen<'t> = (
        &'static str,
        &'t str,
        Option<&'t str>,
        Option<&'static str>,
        Vec<&'static str>,
        Option<&'static str>,
    );

    fn shape(t: &StartTarget) -> Seen<'_> {
        (
            t.form.as_str(),
            t.target.as_str(),
            t.attr.as_deref(),
            t.launcher,
            t.chain.clone(),
            t.refusal(),
        )
    }

    #[test]
    fn the_table_is_valid() {
        assert_eq!(validate_launchers(), Vec::<String>::new());
    }

    #[test]
    fn validate_names_each_problem() {
        let bad = [
            Launcher::new(&["a*b", "x y z"], TargetForm::Script)
                .values(&["q"])
                .inline(&["q"])
                .positional(1),
            Launcher::new(&["w"], TargetForm::Wrapper).lang("python"),
            Launcher::new(&["w"], TargetForm::PyModule),
            Launcher::new(&[], TargetForm::Binary),
        ];
        let problems = validate_launcher_rows(&bad);
        let has = |s: &str| problems.iter().any(|p| p.contains(s));
        assert!(has("may end in `*` only"), "{problems:?}");
        assert!(has("must be one or two words"));
        assert!(has("must start with `-`"));
        assert!(has("both a value and an inline flag"));
        assert!(has("only a wrapper consumes positionals"));
        assert!(has("a wrapper has no language"));
        assert!(has("duplicate name \"w\""));
        assert!(has("py_module is a reader's answer"));
        assert!(has("no names"));
        assert!(has("binary is a reader's answer"));
        assert_eq!(problems.len(), 11, "{problems:?}");
    }

    /// The argvs the packet was specced from: one per public repo line.
    #[test]
    fn the_quoted_starts() {
        // microservices-demo emailservice / currencyservice Dockerfiles.
        let t = norm(&["python", "email_server.py"]);
        assert_eq!(
            shape(&t),
            (
                "script",
                "email_server.py",
                None,
                Some("python"),
                vec![],
                None
            )
        );
        assert_eq!(t.lang, Some("python"));
        let t = norm(&["node", "server.js"]);
        assert_eq!(
            shape(&t),
            ("script", "server.js", None, Some("node"), vec![], None)
        );
        assert_eq!(t.lang, Some("typescript"));
        // prefect scripts/proxy-test/Dockerfile.
        let t = norm(&["uvicorn", "server:app"]);
        assert_eq!(
            shape(&t),
            (
                "module_attr",
                "server",
                Some("app"),
                Some("uvicorn"),
                vec![],
                None
            )
        );
        // plane's worker and api entrypoints.
        let t = norm(&["celery", "-A", "plane", "worker", "-l", "info"]);
        assert_eq!(
            shape(&t),
            ("celery_app", "plane", None, Some("celery"), vec![], None)
        );
        assert_eq!(t.args, argv(&["worker", "-l", "info"]));
        let t = norm(&[
            "sh",
            "-c",
            "exec gunicorn -w \"$GUNICORN_WORKERS\" -k uvicorn.workers.UvicornWorker plane.asgi:application --bind 0.0.0.0:\"${PORT:-8000}\" --max-requests 1200",
        ]);
        assert_eq!(
            shape(&t),
            (
                "module_attr",
                "plane.asgi",
                Some("application"),
                Some("gunicorn"),
                vec!["sh", "exec"],
                None
            )
        );
        // twenty packages/twenty-docker/docker-compose.yml.
        let t = norm(&["yarn", "worker:prod"]);
        assert_eq!(
            shape(&t),
            (
                "pkg_script",
                "worker:prod",
                None,
                Some("yarn"),
                vec![],
                None
            )
        );
        // squidex Dockerfile.
        let t = norm(&["dotnet", "Squidex.dll"]);
        assert_eq!(
            shape(&t),
            ("dll", "Squidex.dll", None, Some("dotnet"), vec![], None)
        );
        assert_eq!(t.lang, Some("csharp"));
        // immich machine-learning: ENTRYPOINT ["tini", "--"] ++ CMD.
        let t = norm(&["tini", "--", "python", "-m", "immich_ml"]);
        assert_eq!(
            shape(&t),
            (
                "py_module",
                "immich_ml",
                None,
                Some("python"),
                vec!["tini"],
                None
            )
        );
        // chatwoot deployment/chatwoot-worker.1.service ExecStart.
        let t = norm(&[
            "/bin/bash",
            "-lc",
            "dotenv bundle exec sidekiq -C config/sidekiq.yml",
        ]);
        assert_eq!(
            shape(&t),
            (
                "framework",
                "sidekiq",
                None,
                Some("sidekiq"),
                vec!["bash", "dotenv", "bundle exec"],
                None
            )
        );
        assert_eq!(t.args, argv(&["-C", "config/sidekiq.yml"]));
    }

    /// One argv per launcher row; the test fails when a row has none.
    #[test]
    fn every_row_reads_its_target() {
        type Case = (
            &'static [&'static str],
            &'static str,
            &'static str,
            Option<&'static str>,
        );
        let cases: &[Case] = &[
            (
                &["python3.11", "-u", "app/main.py", "--x"],
                "script",
                "app/main.py",
                None,
            ),
            (
                &[
                    "/usr/local/bin/node",
                    "--require",
                    "dd-trace/init",
                    "dist/index.js",
                ],
                "script",
                "dist/index.js",
                None,
            ),
            (
                &["deno", "run", "--allow-net", "-c", "deno.json", "main.ts"],
                "script",
                "main.ts",
                None,
            ),
            (
                &["bun", "run", "src/index.ts"],
                "script",
                "src/index.ts",
                None,
            ),
            (
                &["tsx", "watch", "src/server.ts"],
                "script",
                "src/server.ts",
                None,
            ),
            (
                &["ts-node", "-P", "tsconfig.json", "src/main.ts"],
                "script",
                "src/main.ts",
                None,
            ),
            (
                &["ruby", "-I", "lib", "bin/worker.rb"],
                "script",
                "bin/worker.rb",
                None,
            ),
            (
                &["php8.2", "-d", "memory_limit=-1", "artisan", "queue:work"],
                "script",
                "artisan",
                None,
            ),
            (
                &["luajit", "-l", "jit.opt", "main.lua"],
                "script",
                "main.lua",
                None,
            ),
            (
                &["bash", "-e", "scripts/start.sh"],
                "script",
                "scripts/start.sh",
                None,
            ),
            (
                &[
                    "gunicorn",
                    "-b",
                    "0.0.0.0:8000",
                    "-k",
                    "gevent",
                    "app.wsgi:application",
                ],
                "module_attr",
                "app.wsgi",
                Some("application"),
            ),
            (
                &[
                    "uvicorn",
                    "--app-dir",
                    "src",
                    "--factory",
                    "api.main:create_app",
                    "--port",
                    "80",
                ],
                "module_attr",
                "api.main",
                Some("create_app"),
            ),
            (
                &["hypercorn", "--bind", "0.0.0.0:80", "main:app"],
                "module_attr",
                "main",
                Some("app"),
            ),
            (
                &[
                    "daphne",
                    "-b",
                    "0.0.0.0",
                    "-p",
                    "8001",
                    "project.asgi:application",
                ],
                "module_attr",
                "project.asgi",
                Some("application"),
            ),
            (
                &["waitress-serve", "--port=8080", "--call", "app:create_app"],
                "module_attr",
                "app",
                Some("create_app"),
            ),
            (
                &["celery", "--app=proj.celery:app", "beat"],
                "celery_app",
                "proj.celery",
                Some("app"),
            ),
            (
                &[
                    "flask",
                    "--app",
                    "hello:create_app()",
                    "run",
                    "--host",
                    "0.0.0.0",
                ],
                "flask_app",
                "hello",
                Some("create_app"),
            ),
            (
                &["npm", "run", "start:prod", "--", "--port", "3000"],
                "pkg_script",
                "start:prod",
                None,
            ),
            (
                &["yarn", "--cwd", "packages/api", "run", "serve"],
                "pkg_script",
                "serve",
                None,
            ),
            (
                &["pnpm", "--filter", "web", "start"],
                "pkg_script",
                "start",
                None,
            ),
            (&["deno", "task", "dev"], "pkg_script", "dev", None),
            (
                &[
                    "java",
                    "-Xmx512m",
                    "-Dspring.profiles.active=prod",
                    "-jar",
                    "/app/app.jar",
                ],
                "jar",
                "/app/app.jar",
                None,
            ),
            (
                &["dotnet", "exec", "Api.dll", "--urls", "http://+:80"],
                "dll",
                "Api.dll",
                None,
            ),
            (
                &["dumb-init", "--", "node", "server.js"],
                "script",
                "server.js",
                None,
            ),
            (
                &["gosu", "app", "python", "worker.py"],
                "script",
                "worker.py",
                None,
            ),
            (&["/init", "node", "index.js"], "script", "index.js", None),
            (
                &["exec", "-a", "svc", "node", "svc.js"],
                "script",
                "svc.js",
                None,
            ),
            (&["nohup", "ruby", "jobs.rb"], "script", "jobs.rb", None),
            (
                &["poetry", "run", "python", "-m", "app.cli"],
                "py_module",
                "app.cli",
                None,
            ),
            (
                &["uv", "run", "--with", "rich", "python", "main.py"],
                "script",
                "main.py",
                None,
            ),
            (
                &["pipenv", "run", "gunicorn", "site:app"],
                "module_attr",
                "site",
                Some("app"),
            ),
            (
                &["npx", "-y", "tsx", "src/worker.ts"],
                "script",
                "src/worker.ts",
                None,
            ),
            (
                &["bundle", "exec", "rails", "server", "-b", "0.0.0.0"],
                "framework",
                "rails",
                None,
            ),
            (
                &["bundle", "exec", "puma", "-C", "config/puma.rb"],
                "framework",
                "puma",
                None,
            ),
            (
                &["python", "manage.py", "runserver", "0.0.0.0:8000"],
                "framework",
                "manage.py",
                None,
            ),
            (&["sidekiq", "-q", "default"], "framework", "sidekiq", None),
            (
                &["tini", "-g", "--", "env", "A=1", "node", "s.js"],
                "script",
                "s.js",
                None,
            ),
            (
                &["su-exec", "node", "sh", "-c", "node server.js"],
                "script",
                "server.js",
                None,
            ),
            (
                &["dotenv", "-e", ".env", "--", "node", "app.js"],
                "script",
                "app.js",
                None,
            ),
        ];
        let mut covered: Vec<&'static str> = Vec::new();
        for (input, form, target, attr) in cases {
            let t = norm(input);
            assert_eq!(
                (
                    t.form.as_str(),
                    t.target.as_str(),
                    t.attr.as_deref(),
                    t.refusal()
                ),
                (*form, *target, *attr, None),
                "{input:?} -> {t:?}"
            );
            covered.extend(t.chain.iter().copied());
            covered.extend(t.launcher);
        }
        for row in LAUNCHERS {
            assert!(
                row.names
                    .iter()
                    .any(|n| covered.contains(&n.strip_suffix('*').unwrap_or(n))),
                "no case reads launcher {:?}",
                row.names
            );
        }
    }

    #[test]
    fn wrappers_and_shells() {
        let t = norm(&["sh", "-c", "a && exec b"]);
        assert_eq!(
            shape(&t),
            ("binary", "b", None, None, vec!["sh", "exec"], None)
        );
        let t = norm(&["env", "A=1", "node", "s.js"]);
        assert_eq!(
            shape(&t),
            ("script", "s.js", None, Some("node"), vec!["env"], None)
        );
        let t = norm(&["tini", "--", "python", "-m", "x"]);
        assert_eq!(
            shape(&t),
            ("py_module", "x", None, Some("python"), vec!["tini"], None)
        );
        // `python -m` a launcher: re-read as that launcher.
        let t = norm(&["python", "-m", "uvicorn", "app.main:app", "--port", "80"]);
        assert_eq!(
            shape(&t),
            (
                "module_attr",
                "app.main",
                Some("app"),
                Some("uvicorn"),
                vec!["python"],
                None
            )
        );
        assert_eq!(t.args, argv(&["--port", "80"]));
        // Leading assignments, and FLASK_APP from them.
        let t = norm(&["FLASK_APP=web.app", "flask", "run"]);
        assert_eq!(
            shape(&t),
            ("flask_app", "web.app", None, Some("flask"), vec![], None)
        );
        // A plain binary keeps its args.
        let t = norm(&["/bin/server", "--port", "80"]);
        assert_eq!(
            shape(&t),
            ("binary", "/bin/server", None, None, vec![], None)
        );
        assert_eq!(t.args, argv(&["--port", "80"]));
        // java -cp: a main class.
        let t = norm(&["java", "-cp", "app.jar:lib/*", "hipstershop.AdService"]);
        assert_eq!(
            shape(&t),
            (
                "java_main",
                "hipstershop.AdService",
                None,
                Some("java"),
                vec![],
                None
            )
        );
        // Shell chains: the last command of `;` / `&&` / `||` / `&`, the
        // first of its pipeline; redirections, comments and quotes read.
        let cases: &[(&str, &[&str])] = &[
            (
                "cd /app; python app.py 2>&1 | tee /var/log/app.log",
                &["python", "app.py"],
            ),
            (
                "set -e\n# start\nexec node 'dist/main.js' >/dev/null",
                &["exec", "node", "dist/main.js"],
            ),
            (
                "npm ci || true && node \"srv/a b.js\"",
                &["node", "srv/a b.js"],
            ),
            (
                "export X=$(cat v) && exec ruby run.rb &>log",
                &["exec", "ruby", "run.rb"],
            ),
            ("python a.py \\\n  --flag", &["python", "a.py", "--flag"]),
        ];
        for (s, words) in cases {
            assert_eq!(split_shell(s), Some(argv(words)), "{s:?}");
        }
        let t = norm(&["bash", "-c", "cd /app; python app.py 2>&1 | tee x.log"]);
        assert_eq!(
            shape(&t),
            ("script", "app.py", None, Some("python"), vec!["bash"], None)
        );
    }

    #[test]
    fn refusals() {
        let t = norm(&["$APP"]);
        assert_eq!(
            shape(&t),
            ("binary", "$APP", None, None, vec![], Some("dynamic"))
        );
        let t = norm(&["python", "${ENTRY}"]);
        assert_eq!(t.refusal(), Some("dynamic"));
        assert_eq!(t.target, "${ENTRY}");
        // `exec "$@"`: the start's own argv handed on, a later reader's.
        let t = norm(&["sh", "-c", "exec \"$@\""]);
        assert_eq!(
            shape(&t),
            (
                "binary",
                "$@",
                None,
                None,
                vec!["sh", "exec"],
                Some("dynamic")
            )
        );
        let t = norm(&["gunicorn", "$APP_MODULE"]);
        assert_eq!(t.refusal(), Some("dynamic"));
        // What a line scanner does not read.
        for s in [
            "if [ -f x ]; then exec a; else exec b; fi",
            "f() { exec a; }; f",
            "(cd x && exec a)",
            "cat <<EOF\nx\nEOF",
            "exec $(which node) a.js",
            "exec node 'a.js",
            "case $1 in a) x;; esac",
        ] {
            let t = norm(&["sh", "-c", s]);
            assert_eq!(t.refusal(), Some("dynamic"), "{s:?} -> {t:?}");
            assert_eq!(t.form, TargetForm::Shell, "{s:?}");
        }
        // Inline code.
        assert_eq!(
            norm(&["python", "-c", "print(1)"]).refusal(),
            Some("inline")
        );
        assert_eq!(norm(&["node", "-e", "x()"]).refusal(), Some("inline"));
        assert_eq!(norm(&["npx", "-c", "x"]).refusal(), Some("inline"));
        // No target.
        assert_eq!(norm(&[]).refusal(), Some("no_target"));
        assert_eq!(norm(&["A=1"]).refusal(), Some("no_target"));
        assert_eq!(norm(&["exec"]).refusal(), Some("no_target"));
        assert_eq!(norm(&["celery", "worker"]).refusal(), Some("no_target"));
        assert_eq!(norm(&["flask", "run"]).refusal(), Some("no_target"));
        assert_eq!(
            norm(&["yarn", "workspace", "api"]).refusal(),
            Some("no_target")
        );
        assert_eq!(
            norm(&["sh", "-c", "# nothing"]).refusal(),
            Some("no_target")
        );
        // Depth: four hops read, a fifth refused.
        let four = norm(&["tini", "--", "gosu", "u", "env", "nohup", "node", "a.js"]);
        assert_eq!(
            shape(&four),
            (
                "script",
                "a.js",
                None,
                Some("node"),
                vec!["tini", "gosu", "env", "nohup"],
                None
            )
        );
        let five = norm(&[
            "tini", "--", "gosu", "u", "env", "nohup", "exec", "node", "a.js",
        ]);
        assert_eq!(five.refusal(), Some("depth"));
        assert_eq!(five.chain, vec!["tini", "gosu", "env", "nohup"]);
        assert_eq!(five.target, "node");
    }

    #[test]
    fn forms_name_their_rules() {
        let names: Vec<&str> = [
            TargetForm::Script,
            TargetForm::PyModule,
            TargetForm::ModuleAttr,
            TargetForm::CeleryApp,
            TargetForm::FlaskApp,
            TargetForm::PkgScript,
            TargetForm::Jar,
            TargetForm::JavaMain,
            TargetForm::Dll,
            TargetForm::Wrapper,
            TargetForm::Shell,
            TargetForm::Framework,
            TargetForm::Binary,
        ]
        .iter()
        .map(|f| f.as_str())
        .collect();
        assert_eq!(
            names,
            [
                "script",
                "py_module",
                "module_attr",
                "celery_app",
                "flask_app",
                "pkg_script",
                "jar",
                "java_main",
                "dll",
                "wrapper",
                "shell",
                "framework",
                "binary"
            ]
        );
        assert_eq!(Refusal::Depth.as_str(), "depth");
    }
}
