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

use std::collections::BTreeMap;

use glia_core::{Cell, Confidence, Node, NodeId, NodeKindId, RepoId};

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
    /// `<prefix>.<key> = v` (and the prefix's mutator calls).
    Assign,
    /// The keys of a table a `<prefix>` builder call returns.
    Builder,
}

/// One way a host's settings are written: a flavour and its prefixes.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ConfigRule {
    pub flavour: ConfigFlavour,
    pub prefixes: &'static [&'static str],
}

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
        config: &[],
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
    /// Host settings read.
    pub config: usize,
    /// Anonymous handlers minted as FUNCTIONs.
    pub minted: usize,
    /// Host calls that matched a prefix row but register nothing.
    pub aux: usize,
    /// Registrations whose name is not a string literal: nothing minted.
    pub unread: usize,
    /// Matched registrations (read, aux and unread) per host.
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
/// global, a prefix callee with nothing to mint, a shape with an empty key or
/// a 0 / shared argument position, and empty load globs, callback names or
/// config prefixes. Empty = valid.
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
        let Gate::Global(global) = row.gate;
        if global.is_empty() {
            problems.push(format!("{at}: empty gate global"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_valid() {
        assert_eq!(validate(), Vec::<String>::new());
        // Maki first: PR #3's seed.
        let names: Vec<&str> = HOSTS.iter().map(|h| h.name).collect();
        assert_eq!(names, ["maki", "neovim"]);
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
                        prefixes: &["x.o"],
                    },
                    ConfigRule {
                        flavour: ConfigFlavour::Assign,
                        prefixes: &[],
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
        assert_eq!(problems.len(), 11, "{problems:?}");
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
