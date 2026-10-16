//! The TypeScript / JavaScript ethers client reader (DT.10c, DT.10i), with the
//! hardhat, TypeChain and artifact bindings that name the contract (DT.10d,
//! step 7), and the viem / wagmi and web3.js readers (DT.10j, steps 8-9).
//!
//! 1. **Gate**, per library. ethers (steps 2-7): the file imports or
//!    requires `ethers` (`ethers/..` and an `@ethersproject/` path too),
//!    `hardhat` (whose `ethers` is ethers) or a TypeChain output path (a
//!    `typechain` / `typechain-types` segment), or holds a TypeChain
//!    `<C>__factory` identifier. viem (step 8): `viem` / `viem/..`, `wagmi` /
//!    `wagmi/..` / `@wagmi/..`, or `hardhat` (whose `viem` is hardhat-viem).
//!    web3.js (step 9): `web3` / `web3/..`. A reader whose gate is closed
//!    reads nothing, so a local `class Contract` mints nothing, and neither
//!    does a `new Contract` in a viem-only file.
//! 2. **ABI bindings.** `(export )?(const|let|var) <N>(: T)? = <abi literal>`
//!    at any depth, read by `abi::read_abi`. A binding whose literal does not
//!    read is no ABI. An ABI binding is visible file-wide (the nearest one
//!    before a use, else the first).
//! 3. **Contract bindings.** A construction `(await )?new
//!    (ethers.)?(Contract|BaseContract)(<addr>, <abi>, ..)`, optionally
//!    chained through `.connect(..)` / `.attach(..)`, bound by
//!    - `(const|let|var) <x>(: T)? = ..` and a bare `<x> = ..`, key `<x>`;
//!    - a class field `(private|public|protected|readonly|static|..
//!      )*<x>(?|!)?(: T)? = ..` (or `#<x> = ..`), key `this.<x>` (`this.#<x>`);
//!    - `this.<x> = ..` / `this.#<x> = ..` in a constructor or method, key
//!      `this.<x>`.
//!
//!    The ABI argument (an identifier may carry a TS `!` or `as T`): an
//!    artifact import `<A>` or `<A>.abi` (step 7) is `Scope::Contract(<C>)`;
//!    a TypeChain `<C>__factory.abi` is `Scope::Contract(<C>)`; an identifier
//!    with an ABI binding is `Scope::Abi(<identifier>)` with its entries; any
//!    other identifier (an import) `Scope::Abi(<identifier>)` with no
//!    entries, and so is `<J>.abi` for any other `.json` import `J`; an
//!    inline literal that reads `Scope::Abi(<x>)` with its entries; anything
//!    else (a call, a member path) binds nothing and counts `dynamic`. A
//!    rebinding `<y> = R.connect(..)` / `R.attach(..)` (any of the binding
//!    forms above, on a bound receiver `R`) binds `y` to R's scope, entries
//!    and library.
//! 4. **Scope.** A `const` / `let` binding is visible from the end of its
//!    initializer to the end of its innermost enclosing block; a `var`
//!    binding file-wide. A bare assignment binds in the block of the nearest
//!    preceding `let x` declaration that holds it, file-wide after a `var x`
//!    or with none; one in a parameter list or after a type annotation (`x: T
//!    = new ..` outside a class body, a default parameter) binds nothing. A
//!    `this.` binding is visible in its class body only, before its
//!    assignment too (methods run in any order), and never as a bare `<x>`:
//!    a class field is no variable in scope. A site reads the NEAREST binding
//!    of its receiver that precedes it and whose scope holds it (for a
//!    `this.` key with none before it, the first one in the class), so a
//!    later function's parameter `escrow` never reads an earlier function's
//!    contract.
//! 5. **Call sites** on a bound receiver `R` (a bare `R` is never a member,
//!    so `this.escrow` is not `escrow`), after any `.connect(..)` /
//!    `.attach(..)` links:
//!    - `R.<m>(` (and `R?.<m>(`, `R!.<m>(`), `R['<m>'](` / `R["<m>"](`; a
//!      signature key `'m(uint256,address)'` is member `m` whose `args` is
//!      the arity it spells;
//!    - ethers v6 `R.<m>.(staticCall|send|estimateGas|populateTransaction|
//!      staticCallResult)(` and v5 `R.(callStatic|estimateGas|
//!      populateTransaction|functions).<m>(`;
//!    - a chained `new (ethers.)?Contract(<addr>, <abi identifier>, ..)<links>`
//!      followed by any of those, with no binding.
//!
//!    [`ETHERS_MEMBERS`] are never sites and are not counted (the stop list
//!    applies to the member after the chain: `R.connect(s).getAddress()` is
//!    none). With ABI entries known, a member that is no function of them
//!    mints nothing and counts `not_in_abi`. `args` is the depth-0 argument
//!    count of the final call; `lib` is the binding's (`ethers`, or step
//!    7's `hardhat` / `typechain`). A non-literal element key
//!    `R[name](` is no site and counts `dynamic`. A chain on an unbound
//!    receiver is not read.
//! 6. **Event sites** on `R`: `R.(on|once|addListener|queryFilter)('<E>'` (a
//!    signature key `'E(uint256)'` reads as `E`) and every `R.filters.<E>(`
//!    (so `R.on(R.filters.E(), ..)` and `R.queryFilter(R.filters.E(), ..)`
//!    are one site each, at the filter). With entries known, only an event
//!    of them.
//! 7. **Contract-naming bindings (DT.10d).** Each is an ethers contract, so
//!    steps 3-6 read it (links, rebinding, every call form, the stop list):
//!    - hardhat (lib `hardhat`), on `ethers` or `<x>.ethers` (`hre.ethers`):
//!      `getContractFactory('<C>', ..)` binds a FACTORY (any binding form of
//!      step 3, scoped as step 4); `getContractAt('<C>', ..)` and
//!      `deployContract('<C>', ..)` bind `Scope::Contract(<C>)`;
//!      `getContractAt(<abi>, ..)` reads its first argument as a `new
//!      Contract` ABI argument (an inline literal binds `Scope::Abi(<x>)`
//!      with its entries and counts in `abis`). A fully qualified name
//!      `'contracts/E.sol:C'` names `C`.
//!    - TypeChain (lib `typechain`): `<C>__factory.connect(<addr>,
//!      <runner>)`, bound or chained (`.<m>(` straight after), is
//!      `Scope::Contract(<C>)`; `new <C>__factory(<signer>)` is a FACTORY,
//!      bound or used inline.
//!    - A factory `F` (bound or inline, after any `.connect(..)` links, which
//!      keep it a factory): `F.deploy(..)` and `F.attach(..)` bind
//!      `Scope::Contract(<C>)` with F's library, and so do OpenZeppelin
//!      `upgrades.deployProxy(F, ..)` and `upgrades.upgradeProxy(<proxy>, F,
//!      ..)`. Only `attach` (and TypeChain's `connect`) returns the contract
//!      itself, so only they are read chained; the others return a promise.
//!    - Artifact imports: `import <A> from '<..>/<File>.sol/<C>.json'`,
//!      `import * as <A>`, `import { abi as <A> }` / `{ abi }`, `const <A> =
//!      require('..')` (and `require('..').abi`), `const { abi(: <A>)? } =
//!      require('..')`, the hardhat `artifacts/contracts/` and foundry `out/`
//!      layouts alike. Any other `.json` import is an ABI of unknown entries.
//!      Imports are file-wide.
//!
//!    Not read (DT.10h): a chai `expect(tx).to.emit(R, 'E')` (an assertion:
//!    `R` is an argument, no access follows it), `hre.artifacts.readArtifact`
//!    and a TypeChain type annotation alone (`const e: Escrow = ..`).
//! 8. **viem / wagmi (DT.10j, lib `viem`).**
//!    - Option calls: [`VIEM_CALLS`] and [`VIEM_EVENTS`] called bare
//!      `f({..})`, as `f(client, {..})` or as `client.f({..})` (an explicit
//!      `<..>` type argument list may come first; a definition `function
//!      f(..)` / a method `f(..) {` is no call). The first `{` argument's
//!      depth-1 keys are read: `abi` (an identifier, `<J>.abi`, an inline
//!      literal or `parseAbi(..)`, the shorthand `abi`), `functionName` /
//!      `eventName` (a string literal; for an event an array of them, one
//!      site each), `args` (an array literal: its element count; absent, 0;
//!      anything else, or absent beside a spread, unknown) and `address`.
//!      The ABI resolves like a `new Contract` argument (step 3). With no ABI
//!      binding to scope by (the shorthand `abi` bound by no declaration or
//!      import of the file, or an inline literal) the `address` identifier is
//!      the hint when its name key drops an address suffix (`ESCROW_ADDRESS`
//!      is `Scope::Abi(ESCROW_ADDRESS)`, name key `escrow`; a bare `address`
//!      names nothing). A non-literal name, an unreadable ABI and a missing
//!      hint count `dynamic`; a call with no name key (`watchContractEvent`
//!      over every event) is no site. [`VIEM_MULTI`] calls read each element
//!      of their `contracts: [..]` array as one option call (an element that
//!      is no object literal counts `dynamic`). A spread keeps only the
//!      literal keys beside it.
//!    - Instances: `getContract({ address, abi, client })` (its options read
//!      as above), and hardhat-viem `(<x>.)?viem.deployContract('<C>', ..)` /
//!      `getContractAt('<C>', ..)` (`Scope::Contract(<C>)`), bound by any
//!      binding form of step 3 and scoped as step 4; `getContract(..)` is the
//!      instance itself, so it is read chained too. On an instance `R`:
//!      calls `R.(read|write|simulate|estimateGas).<m>(`, whose `args` is the
//!      first argument's array length (none, or an options object first, is
//!      0), and events `R.(watchEvent|getEvents|createEventFilter).<E>(`. No
//!      namespace (`read`, `write`, `address`, `abi`, ..) is a member itself.
//! 9. **web3.js (DT.10j, lib `web3`).** `new <w>(.<x>)*.eth.Contract(<abi>(,
//!    <addr>)?(, ..)?)`, the ABI its FIRST argument (read as step 3's), bound
//!    by any binding form of step 3 or chained. On it, calls
//!    `R.methods.<m>(` / `R.methods['<m>'](` (a signature key spells the
//!    arity), whose `args` is that call's argument count (a `.call()` /
//!    `.send(..)` / `.estimateGas(..)` / `.encodeABI()` after it is no
//!    member), and events `R.events.<E>(`, `R.getPastEvents('<E>'` and
//!    `R.once('<E>'` (`allEvents` names none). No ethers stop list applies:
//!    `R.methods.connect(..)` is the contract's `connect`.

use std::collections::BTreeSet;

use glia_code_domain::evm::{self, AbiEntry, ClientSide, EntryKind, Scope};

use super::Site;
use super::abi::read_abi;
use super::scan::{
    enclosing_block, ident_end, ident_start_before, is_ident, is_ident_byte, matching, prev_non_ws,
    skip_ws, split_args, string_literal, trim, word_hits,
};

/// The client library of a `new Contract` site (steps 3 and 5).
const LIB: &str = "ethers";

/// The client library of a hardhat-ethers helper's site (step 7).
const HARDHAT: &str = "hardhat";

/// The client library of a TypeChain factory's site (step 7).
const TYPECHAIN: &str = "typechain";

/// The client library of a viem / wagmi site (step 8).
const VIEM: &str = "viem";

/// The client library of a web3.js site (step 9).
const WEB3: &str = "web3";

/// The suffix of a TypeChain factory class: `Escrow__factory` (step 7).
const FACTORY_SUFFIX: &str = "__factory";

/// What a source must hold for any gate to open (step 1).
const GATE_NEEDLES: &[&str] = &[
    "ethers",
    "hardhat",
    "typechain",
    FACTORY_SUFFIX,
    "viem",
    "wagmi",
    "web3",
];

/// The hardhat-viem helpers on `(<x>.)?viem` that name the contract (step 8).
const VIEM_HELPERS: &[&str] = &["deployContract", "getContractAt"];

/// viem actions and wagmi hooks / actions whose options name one contract
/// function (step 8).
const VIEM_CALLS: &[&str] = &[
    "readContract",
    "writeContract",
    "writeContractAsync",
    "simulateContract",
    "estimateContractGas",
    "prepareWriteContract",
    "useReadContract",
    "useWriteContract",
    "useSimulateContract",
    "useContractRead",
    "useContractWrite",
    "usePrepareContractWrite",
    "useEstimateContractGas",
];

/// viem actions and wagmi hooks whose options name the events they read
/// (step 8).
const VIEM_EVENTS: &[&str] = &[
    "watchContractEvent",
    "getContractEvents",
    "createContractEventFilter",
    "useWatchContractEvent",
    "useContractEvent",
    "parseEventLogs",
];

/// viem / wagmi batch reads: each element of `contracts: [..]` is one option
/// call (step 8).
const VIEM_MULTI: &[&str] = &[
    "multicall",
    "readContracts",
    "useReadContracts",
    "useContractReads",
];

/// viem instance namespaces whose member is a contract function (step 8).
const VIEM_CALL_NS: &[&str] = &["read", "write", "simulate", "estimateGas"];

/// viem instance namespaces whose member is a contract event (step 8).
const VIEM_EVENT_NS: &[&str] = &["watchEvent", "getEvents", "createEventFilter"];

/// web3.js contract members whose first argument names an event (step 9).
const WEB3_EVENT_MEMBERS: &[&str] = &["getPastEvents", "once"];

/// The web3.js name of "every event": no event site (step 9).
const WEB3_ALL_EVENTS: &str = "allEvents";

/// The hardhat-ethers helpers on `(<x>.)?ethers` (step 7).
const HARDHAT_HELPERS: &[&str] = &["getContractFactory", "getContractAt", "deployContract"];

/// OpenZeppelin `upgrades` helpers that take a factory, and the index of the
/// argument that is the factory (step 7).
const UPGRADES_HELPERS: &[(&str, usize)] = &[("deployProxy", 0), ("upgradeProxy", 1)];

/// ethers `BaseContract` members and runtime methods: never an ABI call site,
/// skipped before the ABI check and never counted.
const ETHERS_MEMBERS: &[&str] = &[
    "connect",
    "attach",
    "getAddress",
    "getDeployedCode",
    "waitForDeployment",
    "deploymentTransaction",
    "deployTransaction",
    "deployed",
    "getFunction",
    "getEvent",
    "interface",
    "target",
    "runner",
    "address",
    "resolvedAddress",
    "signer",
    "provider",
    "filters",
    "on",
    "once",
    "off",
    "emit",
    "addListener",
    "removeListener",
    "removeAllListeners",
    "listeners",
    "listenerCount",
    "queryFilter",
    "fallback",
    "callStatic",
    "functions",
    "estimateGas",
    "populateTransaction",
    "then",
    "toString",
];

/// The members whose first argument names the event they read (step 6).
const EVENT_MEMBERS: &[&str] = &["on", "once", "addListener", "queryFilter"];

/// Chain links that keep the contract and its ABI (steps 3 and 5).
const LINKS: &[&str] = &["connect", "attach"];

/// ethers v5 namespaces: `R.<ns>.<m>(` calls or previews ABI function `m`.
const V5_NAMESPACES: &[&str] = &[
    "callStatic",
    "estimateGas",
    "populateTransaction",
    "functions",
];

/// ethers v6 method members: `R.<m>.<s>(` calls or previews ABI function `m`.
const V6_METHODS: &[&str] = &[
    "staticCall",
    "send",
    "estimateGas",
    "populateTransaction",
    "staticCallResult",
];

/// Field modifiers: a class member may lead with any number of them.
const FIELD_MODIFIERS: &[&str] = &[
    "private",
    "public",
    "protected",
    "readonly",
    "static",
    "declare",
    "override",
    "accessor",
];

/// Most bytes a declaration's type annotation is read over.
const MAX_TYPE: usize = 400;

/// What [`scan`] counted besides its sites.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct TsCounts {
    /// ABI bindings read (step 2, plus inline literals of step 3).
    pub(super) abis: usize,
    /// Members and events left out because the known ABI does not list them.
    pub(super) not_in_abi: usize,
    /// Contract constructions whose ABI argument is neither an identifier nor
    /// a literal that reads, calls through a non-literal element key, and
    /// viem option calls with a non-literal name or no ABI to scope by.
    pub(super) dynamic: usize,
}

/// The client API a contract binding's sites are read in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Api {
    /// ethers (with hardhat-ethers and TypeChain): steps 5 and 6.
    Ethers,
    /// A viem contract instance: step 8.
    Viem,
    /// A web3.js contract: step 9.
    Web3,
}

/// The readers a file opens (step 1).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Gate {
    ethers: bool,
    viem: bool,
    web3: bool,
}

impl Gate {
    fn any(self) -> bool {
        self.ethers || self.viem || self.web3
    }

    fn union(self, other: Gate) -> Gate {
        Gate {
            ethers: self.ethers || other.ethers,
            viem: self.viem || other.viem,
            web3: self.web3 || other.web3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kw {
    Const,
    Let,
    Var,
}

/// One `const` / `let` / `var` declarator.
#[derive(Debug)]
struct Decl {
    kw: Kw,
    kw_at: usize,
    name: String,
    /// The first byte of the initializer after `=`; `None` without one.
    init: Option<usize>,
}

/// One ABI binding (step 2).
#[derive(Debug)]
struct AbiBinding {
    name: String,
    at: usize,
    entries: Vec<AbiEntry>,
}

/// One contract binding (step 3) and the range it is visible in (step 4).
#[derive(Debug)]
struct Binding {
    /// The receiver key: `<x>`, `this.<x>` or `this.#<x>`.
    name: String,
    scope: Scope,
    abi: Option<Vec<AbiEntry>>,
    /// The client library its sites name (steps 5, 7, 8 and 9).
    lib: &'static str,
    /// The API its sites are read in.
    api: Api,
    /// The end of its initializer: a site must come after it, unless
    /// `hoisted`.
    at: usize,
    block: (usize, usize),
    /// A `this.` binding, visible in its whole class body (step 4).
    hoisted: bool,
}

/// What [`scan`] reads every binding against.
struct Ctx<'a> {
    source: &'a str,
    view: &'a str,
    decls: &'a [Decl],
    abis: &'a [AbiBinding],
    imports: &'a [JsonImport],
    /// Every name an import clause binds (step 8's "bound in the file").
    locals: &'a [String],
}

/// One `.json` import or require (step 7): the name it binds and, for a
/// hardhat / foundry artifact, the contract it holds.
#[derive(Debug, PartialEq, Eq)]
struct JsonImport {
    name: String,
    contract: Option<String>,
}

/// What an ABI argument names (steps 3 and 7).
enum AbiRef {
    /// A scope known at the argument: a contract, an ABI binding or import,
    /// with the entries when they are known.
    Named(Scope, Option<Vec<AbiEntry>>),
    /// An inline literal: scoped by the variable it lands in.
    Literal(Vec<AbiEntry>),
    /// Nothing readable.
    Dynamic,
}

/// An expression that gives a contract (steps 3 and 7): a construction, a
/// factory product, a hardhat or TypeChain helper call.
struct Made {
    /// Its first byte, where [`binding_target`] reads the binding from.
    start: usize,
    /// Its final `)`.
    close: usize,
    abi: AbiRef,
    lib: &'static str,
    api: Api,
    /// The expression is the contract itself, not a promise of one, so a
    /// member call chained straight after it is a site.
    sync: bool,
}

/// What a construction or rebinding binds (step 3).
struct Target {
    key: String,
    block: (usize, usize),
    hoisted: bool,
}

/// The ABI argument of a construction.
enum AbiArg {
    Ident(String),
    /// `<x>.abi`.
    Member(String),
    Literal(Vec<AbiEntry>),
    Dynamic,
}

/// One member access after a receiver.
enum Step {
    /// `.m`, `?.m`, `!.m`, `['m']` or `["m(uint256)"]`: the member's offset
    /// and name, the arity a signature key spells, the offset just past it
    /// and whether it was dotted.
    Name {
        at: usize,
        name: String,
        arity: Option<u32>,
        end: usize,
        dotted: bool,
    },
    /// `[<key>]` naming no member: `literal` when the key is a string that
    /// is no member name, else a computed key; `end` is just past the `]`.
    Keyed { literal: bool, end: usize },
}

/// What follows a receiver (steps 5 and 6).
enum Access {
    /// A call of ABI function `member` whose name is at `at`.
    Call {
        member: String,
        at: usize,
        args: Option<u32>,
    },
    /// A subscription, query or filter of event `event`.
    Event { event: String, at: usize },
    /// `R[<expr>](`: a call through a non-literal key.
    Dynamic,
    /// A member access that is no site: a stop-listed member, a property, an
    /// event member without a literal name.
    Other,
}

/// The sites of one TS-family file (module doc) over its code `view`.
pub(super) fn scan(source: &str, view: &str) -> (Vec<Site>, TsCounts) {
    let mut counts = TsCounts::default();
    let mut sites = Vec::new();
    let gate = gated(source, view);
    if !gate.any() {
        return (sites, counts);
    }
    let decls = declarations(view);
    let abis: Vec<AbiBinding> = decls
        .iter()
        .filter_map(|d| {
            let init = d.init?;
            let entries = read_abi(source, view, init)?;
            Some(AbiBinding {
                name: d.name.clone(),
                at: d.kw_at,
                entries,
            })
        })
        .collect();
    counts.abis = abis.len();
    let imports = json_imports(source, view, &decls);
    let locals = import_locals(source, view);
    let ctx = Ctx {
        source,
        view,
        decls: &decls,
        abis: &abis,
        imports: &imports,
        locals: &locals,
    };

    let mut bindings: Vec<Binding> = Vec::new();
    if gate.ethers {
        ethers_constructions(&ctx, &mut sites, &mut counts, &mut bindings);
        // Step 7: the bindings that name the contract.
        let mut factories: Vec<Binding> = Vec::new();
        hardhat_bindings(&ctx, &mut factories, &mut sites, &mut counts, &mut bindings);
        typechain_bindings(&ctx, &mut factories, &mut sites, &mut counts, &mut bindings);
        factory_products(&ctx, &factories, &mut sites, &mut counts, &mut bindings);
    }
    if gate.viem {
        viem_instances(&ctx, &mut sites, &mut counts, &mut bindings);
        viem_sites(&ctx, &mut sites, &mut counts);
    }
    if gate.web3 {
        web3_bindings(&ctx, &mut sites, &mut counts, &mut bindings);
    }
    rebind(source, view, &decls, &mut bindings);

    for receiver in keys(&bindings) {
        for (start, end) in receiver_hits(view, &receiver) {
            let Some(binding) = visible(&bindings, &receiver, start) else {
                continue;
            };
            let Some(access) = access_on(binding.api, source, view, end) else {
                continue;
            };
            record(&mut sites, &mut counts, binding, access);
        }
    }
    sites.sort_by(|a, b| (a.at, &a.member).cmp(&(b.at, &b.member)));
    (sites, counts)
}

/// Step 3: every `new (ethers.)?(Contract|BaseContract)(..)`.
fn ethers_constructions(
    ctx: &Ctx,
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    bindings: &mut Vec<Binding>,
) {
    let (source, view) = (ctx.source, ctx.view);
    let vb = view.as_bytes();
    for new_at in word_hits(view, "new") {
        if new_at > 0 && vb[new_at - 1] == b'.' {
            continue;
        }
        let Some((open, close)) = construction(view, new_at) else {
            continue;
        };
        let arg = split_args(view, open, close).get(1).copied();
        let made = Made {
            start: new_at,
            close,
            abi: abi_ref(ctx, abi_arg(source, view, arg), new_at),
            lib: LIB,
            api: Api::Ethers,
            sync: true,
        };
        bind_made(ctx, made, sites, counts, bindings);
    }
}

/// Step 1: the readers the file's imports and requires open (and a TypeChain
/// factory identifier, which opens ethers on its own).
fn gated(source: &str, view: &str) -> Gate {
    let mut gate = Gate::default();
    if !GATE_NEEDLES.iter().any(|needle| source.contains(needle)) {
        return gate;
    }
    gate.ethers = !factory_idents(view).is_empty();
    let vb = view.as_bytes();
    for kw in ["from", "require", "import"] {
        for at in word_hits(view, kw) {
            if at > 0 && vb[at - 1] == b'.' {
                continue;
            }
            let mut k = skip_ws(vb, at + kw.len());
            if vb.get(k) == Some(&b'(') {
                k = skip_ws(vb, k + 1);
            } else if kw == "require" {
                continue;
            }
            if let Some((spec, _)) = string_literal(source, k) {
                gate = gate.union(module_gate(&spec));
            }
        }
    }
    gate
}

/// The readers an import specifier opens (step 1): ethers for ethers
/// itself, hardhat or a TypeChain output directory (`../typechain-types`,
/// `./typechain/..`); viem for viem, wagmi or hardhat (hardhat-viem); web3
/// for web3.
fn module_gate(spec: &str) -> Gate {
    let is =
        |name: &str| spec == name || spec.strip_prefix(name).is_some_and(|t| t.starts_with('/'));
    Gate {
        ethers: is("ethers")
            || spec == "hardhat"
            || spec.starts_with("@ethersproject/")
            || spec
                .split('/')
                .any(|seg| seg == "typechain" || seg == "typechain-types"),
        viem: is("viem") || is("wagmi") || spec.starts_with("@wagmi/") || spec == "hardhat",
        web3: is("web3"),
    }
}

/// Every `const` / `let` / `var` declarator with a plain identifier name.
fn declarations(view: &str) -> Vec<Decl> {
    let vb = view.as_bytes();
    let mut out = Vec::new();
    for (word, kw) in [("const", Kw::Const), ("let", Kw::Let), ("var", Kw::Var)] {
        for kw_at in word_hits(view, word) {
            if kw_at > 0 && vb[kw_at - 1] == b'.' {
                continue;
            }
            let n_at = skip_ws(vb, kw_at + word.len());
            let Some(n_end) = ident_end(vb, n_at) else {
                continue;
            };
            let mut k = skip_ws(vb, n_end);
            if vb.get(k) == Some(&b':') {
                match type_end(vb, k + 1) {
                    Some(eq) => k = eq,
                    None => {
                        out.push(Decl {
                            kw,
                            kw_at,
                            name: view[n_at..n_end].to_string(),
                            init: None,
                        });
                        continue;
                    }
                }
            }
            let init = (vb.get(k) == Some(&b'=') && !matches!(vb.get(k + 1), Some(b'=' | b'>')))
                .then(|| skip_ws(vb, k + 1));
            out.push(Decl {
                kw,
                kw_at,
                name: view[n_at..n_end].to_string(),
                init,
            });
        }
    }
    out.sort_by_key(|d| d.kw_at);
    out
}

/// The offset of the `=` ending the type annotation that starts at `from`;
/// `None` when the declarator has no initializer.
fn type_end(vb: &[u8], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut k = from;
    while k < vb.len() && k < from + MAX_TYPE {
        match vb[k] {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
            b'>' => depth = depth.saturating_sub(1),
            b'=' if vb.get(k + 1) == Some(&b'>') => k += 1,
            b'=' if depth == 0 && vb.get(k + 1) != Some(&b'=') => return Some(k),
            b';' | b',' if depth == 0 => return None,
            _ => {}
        }
        k += 1;
    }
    None
}

/// `new (ethers.)?(Contract|BaseContract)(` at `new_at`: the offsets of its
/// argument parentheses.
fn construction(view: &str, new_at: usize) -> Option<(usize, usize)> {
    let vb = view.as_bytes();
    let mut k = skip_ws(vb, new_at + "new".len());
    let mut end = ident_end(vb, k)?;
    if &view[k..end] == "ethers" {
        let dot = skip_ws(vb, end);
        if vb.get(dot) != Some(&b'.') {
            return None;
        }
        k = skip_ws(vb, dot + 1);
        end = ident_end(vb, k)?;
    }
    if !matches!(&view[k..end], "Contract" | "BaseContract") {
        return None;
    }
    let open = skip_ws(vb, end);
    if vb.get(open) != Some(&b'(') {
        return None;
    }
    Some((open, matching(view, open)?))
}

/// Classify an ABI argument `arg` (a construction's second, a
/// `getContractAt`'s first, a web3.js construction's first, a viem `abi:`
/// value; steps 3, 7, 8 and 9). An array literal or a viem `parseAbi(..)` /
/// `parseAbiItem(..)` that reads is a literal.
fn abi_arg(source: &str, view: &str, arg: Option<(usize, usize)>) -> AbiArg {
    let Some((s, e)) = arg else {
        return AbiArg::Dynamic;
    };
    let vb = view.as_bytes();
    if let Some(entries) = read_abi(source, view, s) {
        return AbiArg::Literal(entries);
    }
    if vb[s] == b'[' {
        return AbiArg::Dynamic;
    }
    let Some(id_end) = ident_end(vb, s) else {
        return AbiArg::Dynamic;
    };
    let mut rest = view[id_end..e].trim_start();
    // `<x>.abi`: an artifact's or a TypeChain factory's ABI (step 7).
    let mut member = false;
    if let Some(after) = rest.strip_prefix('.') {
        let after = after.trim_start();
        match after.strip_prefix("abi") {
            Some(tail) if !tail.bytes().next().is_some_and(is_ident_byte) => {
                rest = tail.trim_start();
                member = true;
            }
            _ => return AbiArg::Dynamic,
        }
    }
    // A TS non-null `!` or an `as T` cast keeps the identifier.
    let rest = rest.strip_prefix('!').unwrap_or(rest).trim_start();
    let cast = rest
        .strip_prefix("as")
        .is_some_and(|t| t.starts_with(char::is_whitespace));
    if !rest.is_empty() && !cast {
        return AbiArg::Dynamic;
    }
    let name = view[s..id_end].to_string();
    if member {
        AbiArg::Member(name)
    } else {
        AbiArg::Ident(name)
    }
}

/// What ABI argument `arg`, used at `at`, names (steps 3 and 7).
fn abi_ref(ctx: &Ctx, arg: AbiArg, at: usize) -> AbiRef {
    let import = |name: &str| ctx.imports.iter().find(|i| i.name == name);
    match arg {
        AbiArg::Ident(name) => match import(&name).and_then(|i| i.contract.clone()) {
            Some(contract) => AbiRef::Named(Scope::Contract(contract), None),
            None => {
                let abi = abi_lookup(ctx.abis, &name, at);
                AbiRef::Named(Scope::Abi(name), abi)
            }
        },
        AbiArg::Member(name) => {
            if let Some(contract) = factory_contract(&name) {
                return AbiRef::Named(Scope::Contract(contract.to_string()), None);
            }
            match import(&name) {
                Some(JsonImport {
                    contract: Some(contract),
                    ..
                }) => AbiRef::Named(Scope::Contract(contract.clone()), None),
                Some(_) => AbiRef::Named(Scope::Abi(name), None),
                None => AbiRef::Dynamic,
            }
        }
        AbiArg::Literal(entries) => AbiRef::Literal(entries),
        AbiArg::Dynamic => AbiRef::Dynamic,
    }
}

/// Bind or read what `made` gives (steps 3, 5 and 7): a member access
/// straight after a `sync` expression is a chained site on it (an inline
/// literal or an unreadable ABI counts `dynamic` there); with nothing after
/// it, the variable it lands in is a binding.
fn bind_made(
    ctx: &Ctx,
    made: Made,
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    bindings: &mut Vec<Binding>,
) {
    if let Some(access) = access_on(made.api, ctx.source, ctx.view, made.close + 1) {
        if !made.sync || matches!(access, Access::Other) {
            return;
        }
        let AbiRef::Named(scope, abi) = made.abi else {
            counts.dynamic += 1;
            return;
        };
        let chained = Binding {
            name: String::new(),
            scope,
            abi,
            lib: made.lib,
            api: made.api,
            at: made.close,
            block: (0, ctx.view.len()),
            hoisted: false,
        };
        record(sites, counts, &chained, access);
        return;
    }
    let Some(target) = binding_target(ctx.view, ctx.decls, made.start) else {
        return;
    };
    let (scope, abi) = match made.abi {
        AbiRef::Named(scope, abi) => (scope, abi),
        AbiRef::Literal(entries) => {
            counts.abis += 1;
            let base = target.key.rsplit(['.', '#']).next().unwrap_or_default();
            (Scope::Abi(base.to_string()), Some(entries))
        }
        AbiRef::Dynamic => {
            counts.dynamic += 1;
            return;
        }
    };
    bindings.push(Binding {
        name: target.key,
        scope,
        abi,
        lib: made.lib,
        api: made.api,
        at: made.close,
        block: target.block,
        hoisted: target.hoisted,
    });
}

/// Step 7, hardhat: `getContractFactory` factories, `getContractAt` and
/// `deployContract` contracts on `(<x>.)?ethers`.
fn hardhat_bindings(
    ctx: &Ctx,
    factories: &mut Vec<Binding>,
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    bindings: &mut Vec<Binding>,
) {
    let (source, view) = (ctx.source, ctx.view);
    let vb = view.as_bytes();
    for &helper in HARDHAT_HELPERS {
        for at in word_hits(view, helper) {
            let Some(start) = member_of(view, at, "ethers") else {
                continue;
            };
            let Some(open) = paren_after(vb, at + helper.len()) else {
                continue;
            };
            let Some(close) = matching(view, open) else {
                continue;
            };
            let first = split_args(view, open, close).first().copied();
            let named = first.and_then(|(s, e)| contract_name(source, s, e));
            let abi = match (helper, named) {
                (_, Some(contract)) => AbiRef::Named(Scope::Contract(contract), None),
                ("getContractAt", None) => abi_ref(ctx, abi_arg(source, view, first), at),
                _ => continue,
            };
            if helper == "getContractFactory" {
                if access(source, view, close + 1).is_some() {
                    continue;
                }
                let (AbiRef::Named(scope, _), Some(target)) =
                    (abi, binding_target(view, ctx.decls, start))
                else {
                    continue;
                };
                factories.push(Binding {
                    name: target.key,
                    scope,
                    abi: None,
                    lib: HARDHAT,
                    api: Api::Ethers,
                    at: close,
                    block: target.block,
                    hoisted: target.hoisted,
                });
                continue;
            }
            let made = Made {
                start,
                close,
                abi,
                lib: HARDHAT,
                api: Api::Ethers,
                sync: false,
            };
            bind_made(ctx, made, sites, counts, bindings);
        }
    }
}

/// Step 7, TypeChain: `<C>__factory.connect(..)` contracts and `new
/// <C>__factory(..)` factories (a product used inline is read here).
fn typechain_bindings(
    ctx: &Ctx,
    factories: &mut Vec<Binding>,
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    bindings: &mut Vec<Binding>,
) {
    let (source, view) = (ctx.source, ctx.view);
    let vb = view.as_bytes();
    for (at, end, contract) in factory_idents(view) {
        let start = chain_start(view, at);
        let new_at = prev_non_ws(vb, start)
            .and_then(|p| ident_start_before(vb, p + 1))
            .filter(|&w| view[w..start].trim_end() == "new");
        if let Some(new_at) = new_at {
            let Some(close) = paren_after(vb, end).and_then(|open| matching(view, open)) else {
                continue;
            };
            let factory = Binding {
                name: String::new(),
                scope: Scope::Contract(contract),
                abi: None,
                lib: TYPECHAIN,
                api: Api::Ethers,
                at: close,
                block: (0, vb.len()),
                hoisted: false,
            };
            if let Some(made) = product(ctx, &factory, new_at, close + 1) {
                bind_made(ctx, made, sites, counts, bindings);
                continue;
            }
            if access(source, view, close + 1).is_some() {
                continue;
            }
            if let Some(target) = binding_target(view, ctx.decls, new_at) {
                factories.push(Binding {
                    name: target.key,
                    block: target.block,
                    hoisted: target.hoisted,
                    ..factory
                });
            }
            continue;
        }
        let Some(Step::Name {
            name,
            end: n_end,
            dotted: true,
            ..
        }) = step(source, view, end)
        else {
            continue;
        };
        if name != "connect" {
            continue;
        }
        let Some(close) = paren_after(vb, n_end).and_then(|open| matching(view, open)) else {
            continue;
        };
        let made = Made {
            start,
            close,
            abi: AbiRef::Named(Scope::Contract(contract), None),
            lib: TYPECHAIN,
            api: Api::Ethers,
            sync: true,
        };
        bind_made(ctx, made, sites, counts, bindings);
    }
}

/// Step 7: what a bound factory gives, `F.deploy(..)` / `F.attach(..)` and
/// `upgrades.deployProxy(F, ..)` / `upgrades.upgradeProxy(<proxy>, F, ..)`.
fn factory_products(
    ctx: &Ctx,
    factories: &[Binding],
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    bindings: &mut Vec<Binding>,
) {
    let view = ctx.view;
    let vb = view.as_bytes();
    for key in keys(factories) {
        for (start, end) in receiver_hits(view, &key) {
            let Some(factory) = visible(factories, &key, start) else {
                continue;
            };
            if let Some(made) = product(ctx, factory, start, end) {
                bind_made(ctx, made, sites, counts, bindings);
            }
        }
    }
    for &(helper, index) in UPGRADES_HELPERS {
        for at in word_hits(view, helper) {
            let Some(start) = member_of(view, at, "upgrades") else {
                continue;
            };
            let Some(open) = paren_after(vb, at + helper.len()) else {
                continue;
            };
            let Some(close) = matching(view, open) else {
                continue;
            };
            let Some(&(s, e)) = split_args(view, open, close).get(index) else {
                continue;
            };
            if ident_end(vb, s) != Some(e) {
                continue;
            }
            let Some(factory) = visible(factories, &view[s..e], s) else {
                continue;
            };
            let made = Made {
                start,
                close,
                abi: AbiRef::Named(factory.scope.clone(), None),
                lib: factory.lib,
                api: Api::Ethers,
                sync: false,
            };
            bind_made(ctx, made, sites, counts, bindings);
        }
    }
}

/// The contract a factory expression `[start, from)` gives through
/// `.deploy(..)` (a promise) or `.attach(..)` (the contract), past any
/// `.connect(..)` links, which keep a factory; `None` for anything else.
fn product(ctx: &Ctx, factory: &Binding, start: usize, from: usize) -> Option<Made> {
    let (source, view) = (ctx.source, ctx.view);
    let vb = view.as_bytes();
    let mut k = from;
    loop {
        let Step::Name {
            name,
            end,
            dotted: true,
            ..
        } = step(source, view, k)?
        else {
            return None;
        };
        let close = matching(view, paren_after(vb, end)?)?;
        match name.as_str() {
            "connect" => k = close + 1,
            "deploy" | "attach" => {
                return Some(Made {
                    start,
                    close,
                    abi: AbiRef::Named(factory.scope.clone(), None),
                    lib: factory.lib,
                    api: Api::Ethers,
                    sync: name == "attach",
                });
            }
            _ => return None,
        }
    }
}

/// Step 8: hardhat-viem `(<x>.)?viem.deployContract('<C>', ..)` /
/// `getContractAt('<C>', ..)` contracts and viem `getContract({..})`
/// instances.
fn viem_instances(
    ctx: &Ctx,
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    bindings: &mut Vec<Binding>,
) {
    let (source, view) = (ctx.source, ctx.view);
    let vb = view.as_bytes();
    for &helper in VIEM_HELPERS {
        for at in word_hits(view, helper) {
            let Some(start) = member_of(view, at, "viem") else {
                continue;
            };
            let Some(open) = paren_after(vb, at + helper.len()) else {
                continue;
            };
            let Some(close) = matching(view, open) else {
                continue;
            };
            let first = split_args(view, open, close).first().copied();
            let Some(contract) = first.and_then(|(s, e)| contract_name(source, s, e)) else {
                continue;
            };
            let made = Made {
                start,
                close,
                abi: AbiRef::Named(Scope::Contract(contract), None),
                lib: VIEM,
                api: Api::Viem,
                sync: false,
            };
            bind_made(ctx, made, sites, counts, bindings);
        }
    }
    for at in word_hits(view, "getContract") {
        let Some((open, close)) = option_call(view, at, "getContract".len()) else {
            continue;
        };
        let Some(opts) = first_options(source, view, open, close) else {
            continue;
        };
        let made = Made {
            start: chain_start(view, at),
            close,
            abi: options_abi(ctx, &opts, at),
            lib: VIEM,
            api: Api::Viem,
            sync: true,
        };
        bind_made(ctx, made, sites, counts, bindings);
    }
}

/// Step 8: the viem / wagmi option calls ([`VIEM_CALLS`], [`VIEM_EVENTS`])
/// and the batch reads ([`VIEM_MULTI`]).
fn viem_sites(ctx: &Ctx, sites: &mut Vec<Site>, counts: &mut TsCounts) {
    let (source, view) = (ctx.source, ctx.view);
    let vb = view.as_bytes();
    let tables = [
        (VIEM_CALLS, Some(ClientSide::Call)),
        (VIEM_EVENTS, Some(ClientSide::Event)),
        (VIEM_MULTI, None),
    ];
    for (names, side) in tables {
        for &name in names {
            for at in word_hits(view, name) {
                let Some((open, close)) = option_call(view, at, name.len()) else {
                    continue;
                };
                let Some(opts) = first_options(source, view, open, close) else {
                    continue;
                };
                if let Some(side) = side {
                    option_site(ctx, side, &opts, sites, counts);
                    continue;
                }
                let Some((s, _)) = opts.get("contracts") else {
                    continue;
                };
                let Some(list_close) = (vb[s] == b'[').then(|| matching(view, s)).flatten() else {
                    counts.dynamic += 1;
                    continue;
                };
                for (es, ee) in split_args(view, s, list_close) {
                    match (vb[es] == b'{')
                        .then(|| options(source, view, es, ee))
                        .flatten()
                    {
                        Some(element) => {
                            option_site(ctx, ClientSide::Call, &element, sites, counts)
                        }
                        None => counts.dynamic += 1,
                    }
                }
            }
        }
    }
}

/// The parentheses of a call of the identifier at `at` (`len` bytes long),
/// past an explicit `<..>` type argument list; `None` when it is no call or
/// is a definition: `function f(..)`, a method `f(..) {` or `f(..): T {`.
fn option_call(view: &str, at: usize, len: usize) -> Option<(usize, usize)> {
    let vb = view.as_bytes();
    if let Some(p) = prev_non_ws(vb, at)
        && let Some(w) = ident_start_before(vb, p + 1)
        && &view[w..=p] == "function"
    {
        return None;
    }
    let mut k = skip_ws(vb, at + len);
    if vb.get(k) == Some(&b'<') {
        k = skip_ws(vb, type_args_end(vb, k)? + 1);
    }
    if vb.get(k) != Some(&b'(') {
        return None;
    }
    let close = matching(view, k)?;
    let after = skip_ws(vb, close + 1);
    let defined = match vb.get(after) {
        Some(b'{') => true,
        Some(b':') => {
            let t = (after + 1..vb.len().min(after + 1 + MAX_TYPE)).find(|&j| {
                !(is_ident_byte(vb[j])
                    || vb[j].is_ascii_whitespace()
                    || b"<>[]|&.,'\"`".contains(&vb[j]))
            });
            t.is_some_and(|j| vb[j] == b'{')
        }
        _ => false,
    };
    (!defined).then_some((k, close))
}

/// The offset of the `>` closing the type argument list whose `<` is at
/// `open`; `None` when it does not close within [`MAX_TYPE`] bytes or meets
/// something no type holds.
fn type_args_end(vb: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for k in open..vb.len().min(open + MAX_TYPE) {
        match vb[k] {
            b'<' => depth += 1,
            b'>' if k > 0 && vb[k - 1] == b'=' => {}
            b'>' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(k);
                }
            }
            b';' | b'{' | b'}' | b'(' | b')' => return None,
            _ => {}
        }
    }
    None
}

/// The depth-1 keys of one object literal (step 8).
struct Opts {
    /// `(key, value range)` in source order; a shorthand key is its own
    /// value.
    keys: Vec<(String, (usize, usize))>,
    /// The object spreads another (`{ ...cfg, functionName: 'x' }`).
    spread: bool,
}

impl Opts {
    fn get(&self, key: &str) -> Option<(usize, usize)> {
        self.keys.iter().find(|(k, _)| k == key).map(|&(_, v)| v)
    }
}

/// The first `{` argument of the call whose parentheses are `open` /
/// `close`, read as [`Opts`].
fn first_options(source: &str, view: &str, open: usize, close: usize) -> Option<Opts> {
    let vb = view.as_bytes();
    let (s, e) = split_args(view, open, close)
        .into_iter()
        .find(|&(s, _)| vb[s] == b'{')?;
    options(source, view, s, e)
}

/// The object literal `[s, e)` (its `{` at `s`) as [`Opts`]; `None` when its
/// braces do not close at `e`.
fn options(source: &str, view: &str, s: usize, e: usize) -> Option<Opts> {
    let vb = view.as_bytes();
    let close = matching(view, s)?;
    if close + 1 != e && !view[close + 1..e].trim_start().starts_with("as ") {
        return None;
    }
    let mut opts = Opts {
        keys: Vec::new(),
        spread: false,
    };
    for (ps, pe) in split_args(view, s, close) {
        if view[ps..pe].starts_with("...") {
            opts.spread = true;
            continue;
        }
        let (key, key_end) = match ident_end(vb, ps) {
            Some(k_end) => (view[ps..k_end].to_string(), k_end),
            None => match string_literal(source, ps) {
                Some(literal) => literal,
                None => continue,
            },
        };
        let colon = skip_ws(vb, key_end);
        if colon >= pe {
            opts.keys.push((key, (ps, key_end)));
        } else if vb[colon] == b':' {
            let (vs, ve) = trim(vb, colon + 1, pe);
            if vs < ve {
                opts.keys.push((key, (vs, ve)));
            }
        }
    }
    Some(opts)
}

/// The ABI an options object names (step 8): its `abi` value read as a
/// `new Contract` argument, the `address` hint for a shorthand `abi` the
/// file binds nowhere, [`AbiRef::Dynamic`] with no `abi` key.
fn options_abi(ctx: &Ctx, opts: &Opts, at: usize) -> AbiRef {
    let Some(range) = opts.get("abi") else {
        return AbiRef::Dynamic;
    };
    match abi_arg(ctx.source, ctx.view, Some(range)) {
        AbiArg::Ident(name) if name == "abi" && !bound_in_file(ctx, &name) => {
            address_hint(ctx, opts).map_or(AbiRef::Dynamic, |scope| AbiRef::Named(scope, None))
        }
        arg => abi_ref(ctx, arg, at),
    }
}

/// True when a declaration or an import of the file binds `name`.
fn bound_in_file(ctx: &Ctx, name: &str) -> bool {
    ctx.decls.iter().any(|d| d.name == name)
        || ctx.imports.iter().any(|i| i.name == name)
        || ctx.locals.iter().any(|l| l == name)
}

/// The scope an options object's `address` identifier hints (step 8): the
/// identifier, when its name key drops an address suffix (`ESCROW_ADDRESS`,
/// `escrowAddr`); `None` for any other `address` value.
fn address_hint(ctx: &Ctx, opts: &Opts) -> Option<Scope> {
    let range = opts.get("address")?;
    let AbiArg::Ident(name) = abi_arg(ctx.source, ctx.view, Some(range)) else {
        return None;
    };
    let flat: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (evm::name_key(&name) != flat).then_some(Scope::Abi(name))
}

/// The sites of one viem option call `opts` on `side` (step 8).
fn option_site(
    ctx: &Ctx,
    side: ClientSide,
    opts: &Opts,
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
) {
    let (source, view) = (ctx.source, ctx.view);
    let vb = view.as_bytes();
    let key = match side {
        ClientSide::Call => "functionName",
        ClientSide::Event => "eventName",
    };
    let Some((ns, ne)) = opts.get(key) else {
        return;
    };
    let names = match (side, vb[ns]) {
        (ClientSide::Event, b'[') => matching(view, ns).and_then(|close| {
            split_args(view, ns, close)
                .into_iter()
                .map(|(s, e)| literal_name(source, view, s, e).map(|n| (n, s)))
                .collect::<Option<Vec<_>>>()
        }),
        _ => literal_name(source, view, ns, ne).map(|n| vec![(n, ns)]),
    };
    let Some(names) = names.filter(|n| !n.is_empty()) else {
        counts.dynamic += 1;
        return;
    };
    let (scope, abi) = match options_abi(ctx, opts, ns) {
        AbiRef::Named(scope, abi) => (scope, abi),
        AbiRef::Literal(entries) => {
            let Some(scope) = address_hint(ctx, opts) else {
                counts.dynamic += 1;
                return;
            };
            counts.abis += 1;
            (scope, Some(entries))
        }
        AbiRef::Dynamic => {
            counts.dynamic += 1;
            return;
        }
    };
    let args = match opts.get("args") {
        Some((s, _)) if vb[s] == b'[' => matching(view, s)
            .map(|close| split_args(view, s, close).len())
            .and_then(|n| u32::try_from(n).ok()),
        Some(_) => None,
        None => (!opts.spread).then_some(0),
    };
    let binding = Binding {
        name: String::new(),
        scope,
        abi,
        lib: VIEM,
        api: Api::Viem,
        at: ns,
        block: (0, view.len()),
        hoisted: false,
    };
    for (member, at) in names {
        let access = match side {
            ClientSide::Call => Access::Call { member, at, args },
            ClientSide::Event => Access::Event { event: member, at },
        };
        record(sites, counts, &binding, access);
    }
}

/// The member a string literal `[s, e)` names (a trailing `as const` kept
/// out); `None` for anything else.
fn literal_name(source: &str, view: &str, s: usize, e: usize) -> Option<String> {
    let (text, end) = string_literal(source, s)?;
    let rest = view.get(end..e)?.trim();
    if !(rest.is_empty() || rest.starts_with("as ") || rest.starts_with("satisfies ")) {
        return None;
    }
    signature_key(&text).map(|(name, _)| name)
}

/// Step 9: every `new <w>(.<x>)*.eth.Contract(<abi>, ..)`.
fn web3_bindings(
    ctx: &Ctx,
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    bindings: &mut Vec<Binding>,
) {
    let (source, view) = (ctx.source, ctx.view);
    let vb = view.as_bytes();
    for new_at in word_hits(view, "new") {
        if new_at > 0 && vb[new_at - 1] == b'.' {
            continue;
        }
        let Some((open, close)) = web3_construction(view, new_at) else {
            continue;
        };
        let first = split_args(view, open, close).first().copied();
        let made = Made {
            start: new_at,
            close,
            abi: abi_ref(ctx, abi_arg(source, view, first), new_at),
            lib: WEB3,
            api: Api::Web3,
            sync: true,
        };
        bind_made(ctx, made, sites, counts, bindings);
    }
}

/// `new <w>(.<x>)*.eth.Contract(` at `new_at`: the offsets of its argument
/// parentheses.
fn web3_construction(view: &str, new_at: usize) -> Option<(usize, usize)> {
    let vb = view.as_bytes();
    let mut path: Vec<&str> = Vec::new();
    let mut k = skip_ws(vb, new_at + "new".len());
    loop {
        let end = ident_end(vb, k)?;
        path.push(&view[k..end]);
        let next = skip_ws(vb, end);
        if vb.get(next) != Some(&b'.') {
            k = next;
            break;
        }
        k = skip_ws(vb, next + 1);
    }
    if path.len() < 3 || path[path.len() - 2..] != ["eth", "Contract"] {
        return None;
    }
    (vb.get(k) == Some(&b'(')).then_some(())?;
    Some((k, matching(view, k)?))
}

/// The start of `(<x>.)*<object>.<member>` for the member named at `at`;
/// `None` when the member is not read off an identifier `object`.
fn member_of(view: &str, at: usize, object: &str) -> Option<usize> {
    let vb = view.as_bytes();
    let dot = prev_non_ws(vb, at).filter(|&d| vb[d] == b'.')?;
    let o_end = prev_non_ws(vb, dot)? + 1;
    let o_start = ident_start_before(vb, o_end)?;
    (&view[o_start..o_end] == object).then(|| chain_start(view, o_start))
}

/// The start of the dotted identifier path ending with the identifier at
/// `at`: `hre.ethers` from `ethers`, `types.Escrow__factory` from
/// `Escrow__factory`.
fn chain_start(view: &str, at: usize) -> usize {
    let vb = view.as_bytes();
    let mut start = at;
    while let Some(dot) = prev_non_ws(vb, start)
        && vb[dot] == b'.'
        && let Some(e) = prev_non_ws(vb, dot)
        && let Some(s) = ident_start_before(vb, e + 1)
    {
        start = s;
    }
    start
}

/// The contract a string argument `[s, e)` names: `'Escrow'`, or the last
/// part of a fully qualified `'contracts/Escrow.sol:Escrow'`.
fn contract_name(source: &str, s: usize, e: usize) -> Option<String> {
    let (text, end) = string_literal(source, s)?;
    if end != e {
        return None;
    }
    let name = text.rsplit(':').next()?.trim();
    is_ident(name).then(|| name.to_string())
}

/// The contract of a TypeChain factory identifier `<C>__factory`.
fn factory_contract(ident: &str) -> Option<&str> {
    ident
        .strip_suffix(FACTORY_SUFFIX)
        .filter(|contract| is_ident(contract))
}

/// Every TypeChain factory identifier of the view: its start, its end and
/// the contract it names.
fn factory_idents(view: &str) -> Vec<(usize, usize, String)> {
    let vb = view.as_bytes();
    view.match_indices(FACTORY_SUFFIX)
        .filter_map(|(k, _)| {
            let end = k + FACTORY_SUFFIX.len();
            if vb.get(end).is_some_and(|&c| is_ident_byte(c)) {
                return None;
            }
            let start = ident_start_before(vb, end)?;
            let contract = factory_contract(&view[start..end])?;
            Some((start, end, contract.to_string()))
        })
        .collect()
}

/// The contract a hardhat / foundry artifact path names:
/// `<..>/<File>.sol/<C>.json` gives `C`.
fn artifact_contract(spec: &str) -> Option<String> {
    let mut parts = spec.rsplit('/');
    let contract = parts.next()?.strip_suffix(".json")?;
    let file = parts.next()?;
    (file.len() > ".sol".len() && file.ends_with(".sol") && is_ident(contract))
        .then(|| contract.to_string())
}

/// Every `.json` import and require of the file (step 7).
fn json_imports(source: &str, view: &str, decls: &[Decl]) -> Vec<JsonImport> {
    let vb = view.as_bytes();
    let mut out = Vec::new();
    let mut push = |name: &str, spec: &str| {
        if is_ident(name) {
            out.push(JsonImport {
                name: name.to_string(),
                contract: artifact_contract(spec),
            });
        }
    };
    for at in word_hits(view, "import") {
        if at > 0 && vb[at - 1] == b'.' {
            continue;
        }
        let Some((names, spec)) = import_clause(source, view, at + "import".len()) else {
            continue;
        };
        if spec.ends_with(".json") {
            for (imported, local) in names {
                if matches!(imported.as_deref(), None | Some("abi" | "default")) {
                    push(&local, &spec);
                }
            }
        }
    }
    for at in word_hits(view, "require") {
        if at > 0 && vb[at - 1] == b'.' {
            continue;
        }
        let Some(open) = paren_after(vb, at + "require".len()) else {
            continue;
        };
        let q = skip_ws(vb, open + 1);
        let Some((spec, _)) = string_literal(source, q).filter(|(s, _)| s.ends_with(".json"))
        else {
            continue;
        };
        if let Some(d) = decls.iter().find(|d| d.init == Some(at)) {
            push(&d.name, &spec);
        } else if let Some(name) = import_equals(view, at) {
            push(name, &spec);
        } else if let Some(names) = destructured_abi(view, at) {
            for name in names {
                push(&name, &spec);
            }
        }
    }
    out
}

/// TypeScript `import <A> = require(..)` with `require` at `at`: `A`.
fn import_equals(view: &str, at: usize) -> Option<&str> {
    let vb = view.as_bytes();
    let eq = prev_non_ws(vb, at).filter(|&e| vb[e] == b'=')?;
    let a_end = prev_non_ws(vb, eq)? + 1;
    let a_start = ident_start_before(vb, a_end)?;
    let kw_end = prev_non_ws(vb, a_start)? + 1;
    let kw = ident_start_before(vb, kw_end)?;
    (&view[kw..kw_end] == "import").then_some(&view[a_start..a_end])
}

/// Every local name an import clause binds (step 8): any specifier, any
/// module.
fn import_locals(source: &str, view: &str) -> Vec<String> {
    let vb = view.as_bytes();
    let mut out: Vec<String> = word_hits(view, "import")
        .into_iter()
        .filter(|&at| at == 0 || vb[at - 1] != b'.')
        .filter_map(|at| import_clause(source, view, at + "import".len()))
        .flat_map(|(names, _)| names.into_iter().map(|(_, local)| local))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The local names of one import clause, each with the export it binds
/// (`None` for a default or a namespace import: the module's value).
type ImportNames = Vec<(Option<String>, String)>;

/// The names an `import` clause starting at `from` binds (`{ abi as A }` is
/// `(Some("abi"), "A")`), and its specifier; `None` for a type-only import,
/// a dynamic `import(..)` and a side-effect import. A `type` specifier inside
/// the braces binds no value and is left out.
fn import_clause(source: &str, view: &str, from: usize) -> Option<(ImportNames, String)> {
    let vb = view.as_bytes();
    let mut k = skip_ws(vb, from);
    let mut names = Vec::new();
    loop {
        match *vb.get(k)? {
            b'{' => {
                let close = matching(view, k)?;
                for (s, e) in split_args(view, k, close) {
                    let words: Vec<&str> = view[s..e].split_whitespace().collect();
                    match words.as_slice() {
                        ["type", ..] => {}
                        [name] => names.push((Some((*name).to_string()), (*name).to_string())),
                        [name, "as", alias] => {
                            names.push((Some((*name).to_string()), (*alias).to_string()));
                        }
                        _ => {}
                    }
                }
                k = skip_ws(vb, close + 1);
            }
            b'*' => {
                let a = skip_ws(vb, k + 1);
                let a_end = ident_end(vb, a)?;
                if &view[a..a_end] != "as" {
                    return None;
                }
                let n = skip_ws(vb, a_end);
                let n_end = ident_end(vb, n)?;
                names.push((None, view[n..n_end].to_string()));
                k = skip_ws(vb, n_end);
            }
            _ => {
                let end = ident_end(vb, k)?;
                match &view[k..end] {
                    "from" => break,
                    "type" => return None,
                    name => names.push((None, name.to_string())),
                }
                k = skip_ws(vb, end);
            }
        }
        if vb.get(k) == Some(&b',') {
            k = skip_ws(vb, k + 1);
        }
    }
    let from_end = k + "from".len();
    let q = skip_ws(vb, from_end);
    let (spec, _) = string_literal(source, q)?;
    Some((names, spec))
}

/// `(const|let|var) { abi(: <A>)? } = require(..)` with `require` at `at`:
/// the names bound to the `abi` property.
fn destructured_abi(view: &str, at: usize) -> Option<Vec<String>> {
    let vb = view.as_bytes();
    let eq = prev_non_ws(vb, at).filter(|&e| vb[e] == b'=')?;
    let close = prev_non_ws(vb, eq).filter(|&c| vb[c] == b'}')?;
    let open = (0..close).rev().find(|&k| vb[k] == b'{')?;
    if matching(view, open) != Some(close) {
        return None;
    }
    let kw_end = prev_non_ws(vb, open)? + 1;
    let kw = ident_start_before(vb, kw_end)?;
    if !matches!(&view[kw..kw_end], "const" | "let" | "var") {
        return None;
    }
    let names: Vec<String> = split_args(view, open, close)
        .into_iter()
        .filter_map(|(s, e)| {
            let (key, alias) = view[s..e]
                .split_once(':')
                .map_or((&view[s..e], None), |(k, a)| (k, Some(a)));
            (key.trim() == "abi").then(|| alias.unwrap_or("abi").trim().to_string())
        })
        .collect();
    (!names.is_empty()).then_some(names)
}

/// The entries of ABI binding `name` as a use at `at` sees it: the nearest
/// binding before `at`, else the first; `None` when the file binds no ABI of
/// that name.
fn abi_lookup(abis: &[AbiBinding], name: &str, at: usize) -> Option<Vec<AbiEntry>> {
    abis.iter()
        .filter(|a| a.name == name && a.at < at)
        .max_by_key(|a| a.at)
        .or_else(|| abis.iter().find(|a| a.name == name))
        .map(|a| a.entries.clone())
}

/// The member access starting at `from` (after a receiver or a previous
/// access); `None` when none starts there.
fn step(source: &str, view: &str, from: usize) -> Option<Step> {
    let vb = view.as_bytes();
    let mut k = skip_ws(vb, from);
    if vb.get(k) == Some(&b'!') && vb.get(k + 1) != Some(&b'=') {
        k = skip_ws(vb, k + 1);
    }
    if vb.get(k) == Some(&b'?') && vb.get(k + 1) == Some(&b'.') {
        k += 1;
        let after = skip_ws(vb, k + 1);
        if vb.get(after) == Some(&b'[') {
            k = after;
        }
    }
    match vb.get(k)? {
        b'.' => {
            let at = skip_ws(vb, k + 1);
            let end = ident_end(vb, at)?;
            Some(Step::Name {
                at,
                name: view[at..end].to_string(),
                arity: None,
                end,
                dotted: true,
            })
        }
        b'[' => {
            let close = matching(view, k)?;
            let (s, e) = trim(vb, k + 1, close);
            let end = close + 1;
            let Some((text, _)) = string_literal(source, s).filter(|(_, le)| *le == e) else {
                return Some(Step::Keyed {
                    literal: false,
                    end,
                });
            };
            Some(match signature_key(&text) {
                Some((name, arity)) => Step::Name {
                    at: s,
                    name,
                    arity,
                    end,
                    dotted: false,
                },
                None => Step::Keyed { literal: true, end },
            })
        }
        _ => None,
    }
}

/// A member name or a signature key `m(uint256,(address,bytes))`: the name
/// and, for a signature, the arity it spells (depth-0 parameters).
fn signature_key(text: &str) -> Option<(String, Option<u32>)> {
    let text = text.trim();
    let Some(open) = text.find('(') else {
        return is_ident(text).then(|| (text.to_string(), None));
    };
    let name = text[..open].trim();
    let inner = text[open + 1..].strip_suffix(')')?;
    if !is_ident(name) {
        return None;
    }
    let mut depth = 0usize;
    let mut arity = 0u32;
    let mut piece = false;
    for c in inner.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                piece = true;
            }
            ')' | ']' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                if !piece {
                    return None;
                }
                arity += 1;
                piece = false;
            }
            c if !c.is_whitespace() => piece = true,
            _ => {}
        }
    }
    if depth != 0 || (!piece && arity > 0) {
        return None;
    }
    Some((name.to_string(), Some(arity + u32::from(piece))))
}

/// The `(` right after `end`, if one is there.
fn paren_after(vb: &[u8], end: usize) -> Option<usize> {
    let p = skip_ws(vb, end);
    (vb.get(p) == Some(&b'(')).then_some(p)
}

/// The offset just past any `.connect(..)` / `.attach(..)` links starting at
/// `from`; `from` itself when there are none.
fn skip_links(source: &str, view: &str, from: usize) -> usize {
    let vb = view.as_bytes();
    let mut k = from;
    while let Some(Step::Name {
        name,
        end,
        dotted: true,
        ..
    }) = step(source, view, k)
        && LINKS.contains(&name.as_str())
        && let Some(paren) = paren_after(vb, end)
        && let Some(close) = matching(view, paren)
    {
        k = close + 1;
    }
    k
}

/// What follows a receiver ending at `from`, past its links (steps 5 and 6);
/// `None` when no member access follows.
fn access(source: &str, view: &str, from: usize) -> Option<Access> {
    let vb = view.as_bytes();
    let k = skip_links(source, view, from);
    let (at, name, arity, end, dotted) = match step(source, view, k)? {
        Step::Name {
            at,
            name,
            arity,
            end,
            dotted,
        } => (at, name, arity, end, dotted),
        Step::Keyed { literal, end } => {
            let called = !literal && paren_after(vb, end).is_some();
            return Some(if called {
                Access::Dynamic
            } else {
                Access::Other
            });
        }
    };
    let next = step(source, view, end);
    if dotted && name == "filters" {
        return Some(match next {
            Some(Step::Name { at, name, end, .. }) if paren_after(vb, end).is_some() => {
                Access::Event { event: name, at }
            }
            _ => Access::Other,
        });
    }
    if dotted && EVENT_MEMBERS.contains(&name.as_str()) {
        let event = paren_after(vb, end).and_then(|paren| literal_event(source, vb, paren));
        return Some(event.map_or(Access::Other, |event| Access::Event { event, at }));
    }
    if dotted && V5_NAMESPACES.contains(&name.as_str()) {
        match &next {
            Some(Step::Name {
                at,
                name,
                arity,
                end,
                ..
            }) => {
                if let Some(paren) = paren_after(vb, *end) {
                    return Some(call(view, *at, name.clone(), *arity, paren));
                }
            }
            Some(Step::Keyed {
                literal: false,
                end,
            }) if paren_after(vb, *end).is_some() => return Some(Access::Dynamic),
            _ => {}
        }
    }
    if let Some(paren) = paren_after(vb, end) {
        return Some(call(view, at, name, arity, paren));
    }
    if let Some(Step::Name {
        name: method,
        end: m_end,
        dotted: true,
        ..
    }) = next
        && V6_METHODS.contains(&method.as_str())
        && let Some(paren) = paren_after(vb, m_end)
    {
        return Some(call(view, at, name, arity, paren));
    }
    Some(Access::Other)
}

/// What follows a receiver of a binding read in `api` (steps 5, 6, 8 and 9);
/// `None` when no member access follows.
fn access_on(api: Api, source: &str, view: &str, from: usize) -> Option<Access> {
    match api {
        Api::Ethers => access(source, view, from),
        Api::Viem => viem_access(source, view, from),
        Api::Web3 => web3_access(source, view, from),
    }
}

/// What follows a viem instance (step 8): `.<ns>.<m>(` for a call or event
/// namespace, anything else no site.
fn viem_access(source: &str, view: &str, from: usize) -> Option<Access> {
    let vb = view.as_bytes();
    let Step::Name {
        name: ns,
        end,
        dotted: true,
        ..
    } = step(source, view, from)?
    else {
        return Some(Access::Other);
    };
    let call = VIEM_CALL_NS.contains(&ns.as_str());
    if !call && !VIEM_EVENT_NS.contains(&ns.as_str()) {
        return Some(Access::Other);
    }
    Some(match step(source, view, end) {
        Some(Step::Name { at, name, end, .. }) => match paren_after(vb, end) {
            Some(paren) if call => Access::Call {
                member: name,
                at,
                args: viem_args(view, paren),
            },
            Some(_) => Access::Event { event: name, at },
            None => Access::Other,
        },
        Some(Step::Keyed {
            literal: false,
            end,
        }) if paren_after(vb, end).is_some() => Access::Dynamic,
        _ => Access::Other,
    })
}

/// The ABI argument count of a viem instance call whose `(` is at `paren`
/// (step 8): the first argument's array length; 0 with no argument or an
/// options object first; `None` for anything else.
fn viem_args(view: &str, paren: usize) -> Option<u32> {
    let vb = view.as_bytes();
    let close = matching(view, paren)?;
    let n = match split_args(view, paren, close).first() {
        None => 0,
        Some(&(s, _)) => match vb[s] {
            b'[' => split_args(view, s, matching(view, s)?).len(),
            b'{' => 0,
            _ => return None,
        },
    };
    u32::try_from(n).ok()
}

/// What follows a web3.js contract (step 9): `.methods.<m>(`,
/// `.events.<E>(`, `.getPastEvents('<E>'` / `.once('<E>'`, anything else no
/// site.
fn web3_access(source: &str, view: &str, from: usize) -> Option<Access> {
    let vb = view.as_bytes();
    let Step::Name {
        at: ns_at,
        name: ns,
        end,
        dotted: true,
        ..
    } = step(source, view, from)?
    else {
        return Some(Access::Other);
    };
    let next = step(source, view, end);
    Some(match ns.as_str() {
        "methods" => match next {
            Some(Step::Name {
                at,
                name,
                arity,
                end,
                ..
            }) => match paren_after(vb, end) {
                Some(paren) => Access::Call {
                    member: name,
                    at,
                    args: arity.or_else(|| arg_count(view, paren)),
                },
                None => Access::Other,
            },
            Some(Step::Keyed {
                literal: false,
                end,
            }) if paren_after(vb, end).is_some() => Access::Dynamic,
            _ => Access::Other,
        },
        "events" => match next {
            Some(Step::Name { at, name, end, .. })
                if name != WEB3_ALL_EVENTS && paren_after(vb, end).is_some() =>
            {
                Access::Event { event: name, at }
            }
            _ => Access::Other,
        },
        m if WEB3_EVENT_MEMBERS.contains(&m) => paren_after(vb, end)
            .and_then(|paren| literal_event(source, vb, paren))
            .filter(|event| event != WEB3_ALL_EVENTS)
            .map_or(Access::Other, |event| Access::Event { event, at: ns_at }),
        _ => Access::Other,
    })
}

/// The depth-0 argument count of the call whose `(` is at `paren`.
fn arg_count(view: &str, paren: usize) -> Option<u32> {
    matching(view, paren)
        .map(|close| split_args(view, paren, close).len())
        .and_then(|n| u32::try_from(n).ok())
}

/// A call of `member` (named at `at`) whose final `(` is at `paren`: a site
/// unless the member is stop-listed. `args` is a signature key's spelled
/// arity, else the final call's depth-0 argument count.
fn call(view: &str, at: usize, member: String, arity: Option<u32>, paren: usize) -> Access {
    if ETHERS_MEMBERS.contains(&member.as_str()) {
        return Access::Other;
    }
    let args = arity.or_else(|| arg_count(view, paren));
    Access::Call { member, at, args }
}

/// The event a string first argument after `paren` names (`'E'` or
/// `'E(uint256)'`); `None` for any other first argument.
fn literal_event(source: &str, vb: &[u8], paren: usize) -> Option<String> {
    let q = skip_ws(vb, paren + 1);
    let (text, _) = string_literal(source, q)?;
    signature_key(&text).map(|(name, _)| name)
}

/// The variable a construction or rebinding whose initializer starts at
/// `start` binds, and the range it is visible in (steps 3 and 4); `None`
/// when it binds none.
fn binding_target(view: &str, decls: &[Decl], start: usize) -> Option<Target> {
    let vb = view.as_bytes();
    let mut init = start;
    let mut eq = prev_non_ws(vb, start)?;
    if let Some(s) = ident_start_before(vb, eq + 1)
        && &view[s..=eq] == "await"
    {
        init = s;
        eq = prev_non_ws(vb, s)?;
    }
    if let Some(d) = decls.iter().find(|d| d.init == Some(init)) {
        let block = match d.kw {
            Kw::Var => (0, vb.len()),
            Kw::Const | Kw::Let => enclosing_block(view, d.kw_at),
        };
        return Some(Target {
            key: d.name.clone(),
            block,
            hoisted: false,
        });
    }
    if vb[eq] != b'=' || eq == 0 || b"=!<>+-*/%&|^?".contains(&vb[eq - 1]) {
        return None;
    }
    // A class field: the statement right in a class body.
    let (open, close) = enclosing_block(view, eq);
    if close < vb.len() && is_class_body(view, open) {
        let name = class_field(view, open, eq)?;
        return Some(Target {
            key: format!("this.{name}"),
            block: (open, close),
            hoisted: true,
        });
    }
    let x_end = prev_non_ws(vb, eq)? + 1;
    let x_start = ident_start_before(vb, x_end)?;
    let name = &view[x_start..x_end];
    if let Some(before) = prev_non_ws(vb, x_start) {
        if matches!(vb[before], b'.' | b'#') {
            return this_target(view, before, x_start, name);
        }
        if matches!(vb[before], b'(' | b',' | b':') {
            return None;
        }
        if let Some(w) = ident_start_before(vb, before + 1)
            && FIELD_MODIFIERS.contains(&&view[w..=before])
        {
            return None;
        }
    }
    let block = decls
        .iter()
        .filter(|d| d.name == name && d.kw_at < x_start && d.kw != Kw::Const)
        .filter_map(|d| match d.kw {
            Kw::Var => Some((d.kw_at, (0, vb.len()))),
            _ => {
                let b = enclosing_block(view, d.kw_at);
                (b.0 <= x_start && x_start <= b.1).then_some((d.kw_at, b))
            }
        })
        .max_by_key(|&(at, _)| at)
        .map_or((0, vb.len()), |(_, b)| b);
    Some(Target {
        key: name.to_string(),
        block,
        hoisted: false,
    })
}

/// `this.<name> = ..` / `this.#<name> = ..` in a constructor or method, the
/// `.` or `#` before the name at `before`: key `this.<name>` scoped to the
/// enclosing class body; `None` for any other object or outside a class.
fn this_target(view: &str, before: usize, x_start: usize, name: &str) -> Option<Target> {
    let vb = view.as_bytes();
    let hash = vb[before] == b'#';
    let dot = if hash { before.checked_sub(1)? } else { before };
    if vb[dot] != b'.' {
        return None;
    }
    let this_end = prev_non_ws(vb, dot)? + 1;
    let this_start = ident_start_before(vb, this_end)?;
    if &view[this_start..this_end] != "this"
        || prev_non_ws(vb, this_start).is_some_and(|p| matches!(vb[p], b'.' | b'#'))
    {
        return None;
    }
    let block = class_body(view, x_start)?;
    Some(Target {
        key: format!("this.{}{name}", if hash { "#" } else { "" }),
        block,
        hoisted: true,
    })
}

/// The field a class-body statement ending in the `=` at `eq` (the class
/// body opening at `open`) declares: `(<modifier> )*(#)?<x>(?|!)?(: T)?`, as
/// `x` or `#x`; `None` for any other statement. Inside a parameter list
/// (a constructor's, a method's, an arrow field's) only a parameter property,
/// one with a modifier, is a field: a plain default parameter binds nothing.
fn class_field(view: &str, open: usize, eq: usize) -> Option<String> {
    let vb = view.as_bytes();
    let head = &vb[open + 1..eq];
    let depth = head.iter().fold(0usize, |d, &c| match c {
        b'(' | b'[' => d + 1,
        b')' | b']' => d.saturating_sub(1),
        _ => d,
    });
    let in_params = depth > 0;
    let start = head
        .iter()
        .rposition(|&c| {
            matches!(c, b';' | b'{' | b'}' | b'\n') || (in_params && matches!(c, b'(' | b','))
        })
        .map_or(open + 1, |p| open + 1 + p + 1);
    let mut k = skip_ws(vb, start);
    let mut modified = false;
    loop {
        let hash = vb.get(k) == Some(&b'#');
        let n_at = k + usize::from(hash);
        let n_end = ident_end(vb, n_at)?;
        let word = &view[n_at..n_end];
        let mut j = skip_ws(vb, n_end);
        let more = vb.get(j) == Some(&b'#') || ident_end(vb, j).is_some();
        if !hash && FIELD_MODIFIERS.contains(&word) && more {
            modified = true;
            k = j;
            continue;
        }
        if matches!(vb.get(j), Some(b'?' | b'!')) {
            j = skip_ws(vb, j + 1);
        }
        if vb.get(j) == Some(&b':') {
            j = type_end(vb, j + 1)?;
        }
        let field = j == eq && (modified || !in_params);
        return field.then(|| format!("{}{word}", if hash { "#" } else { "" }));
    }
}

/// True when the `{` at `open` opens a class body: its statement header
/// holds the word `class`.
fn is_class_body(view: &str, open: usize) -> bool {
    let vb = view.as_bytes();
    let start = vb[..open]
        .iter()
        .rposition(|&c| matches!(c, b';' | b'{' | b'}'))
        .map_or(0, |p| p + 1);
    view[start..open]
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .any(|w| w == "class")
}

/// The innermost class body holding `at`; `None` outside every class.
fn class_body(view: &str, at: usize) -> Option<(usize, usize)> {
    let mut pos = at;
    loop {
        let (open, close) = enclosing_block(view, pos);
        if close >= view.len() {
            return None;
        }
        if is_class_body(view, open) {
            return Some((open, close));
        }
        if open == 0 {
            return None;
        }
        pos = open;
    }
}

/// Rebindings `<y> = R.connect(..)` / `R.attach(..)` on a bound ethers
/// receiver (step 3), followed until no new one binds (`y` may itself be
/// rebound).
fn rebind(source: &str, view: &str, decls: &[Decl], bindings: &mut Vec<Binding>) {
    let mut rebound: BTreeSet<usize> = BTreeSet::new();
    loop {
        let mut added = Vec::new();
        for receiver in keys(bindings) {
            for (start, end) in receiver_hits(view, &receiver) {
                if rebound.contains(&start) {
                    continue;
                }
                let tail = skip_links(source, view, end);
                if tail == end || step(source, view, tail).is_some() {
                    continue;
                }
                let Some(from) =
                    visible(bindings, &receiver, start).filter(|b| b.api == Api::Ethers)
                else {
                    continue;
                };
                let Some(target) = binding_target(view, decls, start) else {
                    continue;
                };
                rebound.insert(start);
                added.push(Binding {
                    name: target.key,
                    scope: from.scope.clone(),
                    abi: from.abi.clone(),
                    lib: from.lib,
                    api: from.api,
                    at: tail - 1,
                    block: target.block,
                    hoisted: target.hoisted,
                });
            }
        }
        if added.is_empty() {
            return;
        }
        bindings.extend(added);
    }
}

/// The distinct receiver keys of `bindings`, sorted.
fn keys(bindings: &[Binding]) -> Vec<String> {
    let set: BTreeSet<&str> = bindings.iter().map(|b| b.name.as_str()).collect();
    set.into_iter().map(str::to_string).collect()
}

/// Every place receiver `key` stands in the view, as its `(start, end)`: a
/// bare `<x>` not itself a member (so `this.escrow` is not `escrow`), or
/// `this.<x>` / `this.#<x>`.
fn receiver_hits(view: &str, key: &str) -> Vec<(usize, usize)> {
    let vb = view.as_bytes();
    let ident = key.strip_prefix("this.").unwrap_or(key);
    let ident = ident.strip_prefix('#').unwrap_or(ident);
    let lead = &key.as_bytes()[..key.len() - ident.len()];
    word_hits(view, ident)
        .into_iter()
        .filter_map(|at| {
            let end = at + ident.len();
            let s = at.checked_sub(lead.len())?;
            if vb.get(s..at) != Some(lead) {
                return None;
            }
            let free = s == 0 || !(is_ident_byte(vb[s - 1]) || matches!(vb[s - 1], b'.' | b'#'));
            free.then_some((s, end))
        })
        .collect()
}

/// The binding of `receiver` a site at `at` reads (step 4).
fn visible<'a>(bindings: &'a [Binding], receiver: &str, at: usize) -> Option<&'a Binding> {
    let held = || {
        bindings
            .iter()
            .filter(move |b| b.name == receiver && b.block.0 <= at && at <= b.block.1)
    };
    held()
        .filter(|b| b.at < at)
        .max_by_key(|b| b.at)
        .or_else(|| held().filter(|b| b.hoisted).min_by_key(|b| b.at))
}

/// The site (or count) one access on `binding` gives.
fn record(sites: &mut Vec<Site>, counts: &mut TsCounts, binding: &Binding, access: Access) {
    match access {
        Access::Call { member, at, args } => push_call(sites, counts, binding, member, args, at),
        Access::Event { event, at } => push_event(sites, counts, binding, event, at),
        Access::Dynamic => counts.dynamic += 1,
        Access::Other => {}
    }
}

/// A call site of `member` on `binding`, named at `at` (step 5).
fn push_call(
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    binding: &Binding,
    member: String,
    args: Option<u32>,
    at: usize,
) {
    if let Some(entries) = &binding.abi
        && !entries
            .iter()
            .any(|e| e.kind == EntryKind::Fn && e.name == member)
    {
        counts.not_in_abi += 1;
        return;
    }
    sites.push(Site {
        side: ClientSide::Call,
        scope: binding.scope.clone(),
        member,
        args,
        lib: binding.lib,
        abi: binding.abi.clone(),
        at,
    });
}

/// An event site of `event` on `binding`, read at `at` (step 6).
fn push_event(
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    binding: &Binding,
    event: String,
    at: usize,
) {
    if let Some(entries) = &binding.abi
        && !entries
            .iter()
            .any(|en| en.kind == EntryKind::Event && en.name == event)
    {
        counts.not_in_abi += 1;
        return;
    }
    sites.push(Site {
        side: ClientSide::Event,
        scope: binding.scope.clone(),
        member: event,
        args: None,
        lib: binding.lib,
        abi: binding.abi.clone(),
        at,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evm::scan::code_view;
    use glia_code_domain::evm::{ClientFacts, read_client_cells};

    /// `(side, qname-ish key, args, abi keys)` per site, in file order.
    fn read(src: &str) -> (Vec<String>, TsCounts) {
        let (sites, counts) = scan(src, &code_view(src, "typescript"));
        let rows = sites
            .iter()
            .map(|s| {
                let side = match s.side {
                    ClientSide::Call => "call",
                    ClientSide::Event => "event",
                };
                let abi = s.abi.as_ref().map_or_else(
                    || "-".to_string(),
                    |e| e.iter().map(AbiEntry::to_key).collect::<Vec<_>>().join(","),
                );
                let args = s.args.map_or_else(|| "-".to_string(), |a| a.to_string());
                format!(
                    "{side} {}.{} args={args} abi={abi} line={}",
                    s.scope.key(),
                    s.member,
                    line(src, s.at)
                )
            })
            .collect();
        (rows, counts)
    }

    fn line(src: &str, at: usize) -> usize {
        src[..at].matches('\n').count() + 1
    }

    /// `<scope key>.<member> <lib>` per site, in file order.
    fn libs(src: &str) -> Vec<String> {
        let (sites, _) = scan(src, &code_view(src, "typescript"));
        sites
            .iter()
            .map(|s| format!("{}.{} {}", s.scope.key(), s.member, s.lib))
            .collect()
    }

    const ABI: &str = "const ESCROW_ABI = [
  'function deposit(bytes32 orderId) payable',
  'function release(bytes32 orderId)',
  'event Released(bytes32 indexed orderId, address seller)',
];
";

    #[test]
    fn new_contract_with_same_file_abi() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
export async function pay(addr: string, signer: any, id: string) {{
  const escrow: Contract = await new Contract(addr, ESCROW_ABI, signer);
  return escrow.deposit(id, {{ value: 1n }});
}}
"
        );
        let (rows, counts) = read(&src);
        assert_eq!(
            rows,
            ["call abi:ESCROW_ABI.deposit args=2 abi=f:deposit/1,f:release/1,e:Released/2 line=9"]
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 0,
                dynamic: 0
            }
        );
    }

    #[test]
    fn stop_listed_and_not_in_abi_members_mint_nothing() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
function f(addr: string, p: any) {{
  const escrow = new Contract(addr, ESCROW_ABI, p);
  escrow.getAddress();
  escrow.connect(p).release(addr);
  escrow.waitForDeployment();
  escrow.interface.parseLog(x);
  escrow.withdraw(addr);
  const tx = escrow.release(addr);
  tx.wait();
}}
"
        );
        let (rows, counts) = read(&src);
        // DT.10i: a `.connect(p)` link keeps the contract, so line 10 is a
        // site; the stop list reads the member after the chain.
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=10",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=14"
            ]
        );
        assert_eq!(counts.not_in_abi, 1, "withdraw only");
    }

    #[test]
    fn imported_abi_has_no_entries() {
        let src = "import { Contract } from 'ethers';
import { ESCROW_ABI } from './abi';
const escrow = new Contract(addr, ESCROW_ABI as any, p);
escrow.anything(1, 2);
const dyn = new Contract(addr, loadAbi(), p);
dyn.deposit(1);
const other = new Contract(addr, artifacts.Escrow.abi, p);
";
        let (rows, counts) = read(src);
        assert_eq!(rows, ["call abi:ESCROW_ABI.anything args=2 abi=- line=4"]);
        assert_eq!(
            counts,
            TsCounts {
                abis: 0,
                not_in_abi: 0,
                dynamic: 2
            }
        );
    }

    #[test]
    fn nearest_binding_wins() {
        let src = "const { ethers } = require('ethers');
const A_ABI = ['function a()'];
const B_ABI = ['function b()'];
async function main() {
  let c = new ethers.Contract(x, A_ABI, p);
  c.a();
  c = new ethers.Contract(x, B_ABI, p);
  c.b();
  c.a();
}
";
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call abi:A_ABI.a args=0 abi=f:a/0 line=6",
                "call abi:B_ABI.b args=0 abi=f:b/0 line=8"
            ]
        );
        assert_eq!(counts.not_in_abi, 1, "the second c.a() reads B_ABI");
    }

    #[test]
    fn binding_does_not_leak_out_of_its_block() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
export function releaseNow(address: string, signer: any, id: string) {{
  if (id) {{
    const escrow = new Contract(address, ESCROW_ABI, signer);
    escrow.release(id);
    this.escrow.release(id);
    this.#escrow.release(id);
  }}
  escrow.deposit(id);
}}

export function noteRelease(escrow: {{ release(id: string): string }}, id: string) {{
  return escrow.release(id);
}}
"
        );
        let (rows, _) = read(&src);
        assert_eq!(
            rows,
            ["call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=10"]
        );
    }

    #[test]
    fn bare_assignment_binds_in_its_declaration_block() {
        let src = format!(
            "const {{ ethers }} = require('hardhat');\n{ABI}\
describe('Escrow', () => {{
  let escrow;
  beforeEach(async () => {{
    escrow = new ethers.Contract(addr, ESCROW_ABI, signer);
  }});
  it('deposits', async () => {{
    await escrow.deposit(id);
  }});
}});
describe('Other', () => {{
  it('releases', async () => {{
    await escrow.release(id);
  }});
}});
class Holder {{
  escrow = new ethers.Contract(addr, ESCROW_ABI, signer);
  private vault = new ethers.Contract(addr, ESCROW_ABI, signer);
  go() {{ return vault.release(id); }}
}}
"
        );
        let (rows, _) = read(&src);
        assert_eq!(
            rows,
            ["call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=13"]
        );
    }

    #[test]
    fn chained_new_contract_call() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
const held = (a: string) => new Contract(a, ESCROW_ABI, p).deposit(a);
const linked = (a: string) => new Contract(a, ESCROW_ABI, p).connect(s).release(a);
const gone = (a: string) => new Contract(a, ESCROW_ABI, p).withdraw(a);
const inline = (a: string) => new Contract(a, ['function f()'], p).f();
"
        );
        let (rows, counts) = read(&src);
        // DT.10i: a construction chained through `.connect(s)` is a site.
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=7",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=8"
            ]
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 1,
                dynamic: 1
            }
        );
    }

    #[test]
    fn on_and_once_events() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
function watch(p: any) {{
  const escrow = new Contract(a, ESCROW_ABI, p);
  escrow.on('Released', (id: string) => id);
  escrow.once(\"Released(bytes32,address)\", () => {{}});
  escrow.on('Funded', () => {{}});
  escrow.on(escrow.filters.Released(), () => {{}});
  const loose = new Contract(a, IMPORTED_ABI, p);
  loose.once('Anything', () => {{}});
}}
"
        );
        let (rows, counts) = read(&src);
        assert_eq!(
            rows,
            [
                "event abi:ESCROW_ABI.Released args=- abi=f:deposit/1,f:release/1,e:Released/2 line=9",
                "event abi:ESCROW_ABI.Released args=- abi=f:deposit/1,f:release/1,e:Released/2 line=10",
                // DT.10i: the filter is the site, once.
                "event abi:ESCROW_ABI.Released args=- abi=f:deposit/1,f:release/1,e:Released/2 line=12",
                "event abi:IMPORTED_ABI.Anything args=- abi=- line=14",
            ]
        );
        assert_eq!(counts.not_in_abi, 1, "Funded");
    }

    #[test]
    fn comments_and_strings_never_match() {
        // Kina demo_trade.js: `step("seller calls escrow.createTrade")`.
        let src = format!(
            "const {{ ethers }} = require(\"ethers\");\n{ABI}\
async function main() {{
  const escrow = new ethers.Contract(dep.address, ESCROW_ABI, wallet);
  step(\"seller calls escrow.deposit\");
  // escrow.release(id) would revert
  /* escrow.release(id) */
  const msg = `escrow.deposit(${{escrow.release(id)}})`;
  const re = /escrow.release\\(/;
}}
"
        );
        let (rows, _) = read(&src);
        assert!(rows.is_empty(), "{rows:?}");
    }

    #[test]
    fn ungated_file_mints_nothing() {
        let src = "class Contract {
  constructor(private a: string, private b: string[], private c: unknown) {}
  deposit(x: string) { return x; }
}
const ABI = ['function deposit(bytes32)'];
export function localDeposit(id: string) {
  const escrow = new Contract('0x0', ABI, null);
  return escrow.deposit(id);
}
// import { Contract } from 'ethers';
";
        let (rows, counts) = read(src);
        assert!(rows.is_empty());
        assert_eq!(counts, TsCounts::default());
        let hh = "import { ethers } from 'hardhat';\nconst c = new ethers.Contract(a, ['function f()'], s);\nc.f();\n";
        assert_eq!(read(hh).0, ["call abi:c.f args=0 abi=f:f/0 line=3"]);
        let ep = "import { Contract } from '@ethersproject/contracts';\nconst c = new Contract(a, ['function f()'], s);\nc.f();\n";
        assert_eq!(read(ep).0.len(), 1);
    }

    #[test]
    fn element_access_and_signature_key() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
async function pay(addr: string, signer: any, id: string) {{
  const escrow = new Contract(addr, ESCROW_ABI, signer);
  await escrow['deposit'](id, {{ value: 1n }});
  await escrow[\"release(bytes32)\"](id, {{ gasLimit: 1 }});
  await escrow?.[`release`](id);
  await escrow['withdraw'](id);
  await escrow['getAddress']();
  await escrow['release(bytes32'](id);
  const pick = escrow['deposit'];
  const vault = new Contract(addr, VAULT_ABI, signer);
  await vault['settle((uint256,address),bytes32[])'](a, b, {{ gasLimit: 1 }});
  await vault['settle()']();
}}
"
        );
        let (rows, counts) = read(&src);
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ABI.deposit args=2 abi=f:deposit/1,f:release/1,e:Released/2 line=9",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=10",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=11",
                "call abi:VAULT_ABI.settle args=2 abi=- line=17",
                "call abi:VAULT_ABI.settle args=0 abi=- line=18",
            ],
            "a signature key's spelled arity is the site's args"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 1,
                dynamic: 0
            },
            "withdraw is no ABI function; a stop-listed key and a malformed signature count nothing"
        );
        assert_eq!(
            signature_key(" f(uint256, address) "),
            Some(("f".to_string(), Some(2)))
        );
        assert_eq!(signature_key("f(uint256,)"), None);
        assert_eq!(signature_key("f(uint256))"), None);
        assert_eq!(signature_key("1f"), None);
    }

    #[test]
    fn non_literal_key_is_dynamic() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
function call(addr: string, p: any, name: string) {{
  const escrow = new Contract(addr, ESCROW_ABI, p);
  escrow[name](addr);
  escrow[`${{name}}`](addr);
  escrow.callStatic[name](addr);
  escrow.connect(p)[name](addr);
  const f = escrow[name];
  other[name](addr);
}}
"
        );
        let (rows, counts) = read(&src);
        assert!(rows.is_empty(), "{rows:?}");
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 0,
                dynamic: 4
            },
            "four calls through a computed key on a bound receiver; an uncalled one and an unbound receiver count nothing"
        );
    }

    #[test]
    fn connect_and_attach_chains_and_rebinding() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
async function settle(addr: string, p: any, s: any, id: string) {{
  const escrow = new Contract(addr, ESCROW_ABI, p);
  await escrow.connect(s).release(id);
  await escrow.attach(addr).connect(s).deposit(id);
  await escrow.connect(s).getAddress();
  await escrow.connect(s).withdraw(id);
  const signed = escrow.connect(s);
  await signed.release(id);
  let moved;
  moved = await signed.attach(addr);
  moved.deposit(id);
  await stranger.connect(s).release(id);
  const linked = new Contract(addr, ESCROW_ABI, p).connect(s);
  linked.release(id);
}}
function later(signed: any, id: string) {{
  return signed.release(id);
}}
"
        );
        let (rows, counts) = read(&src);
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=9",
                "call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=10",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=14",
                "call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=17",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=20",
            ],
            "a rebinding is block-scoped: `later`'s parameter is no contract"
        );
        assert_eq!(
            counts.not_in_abi, 1,
            "withdraw only: getAddress is stop-listed"
        );
        assert_eq!(counts.dynamic, 0);
    }

    #[test]
    fn class_field_and_this_assignment_bindings_scope_to_the_class() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
const TOKEN_ABI = ['function decimals() view returns (uint8)'];
export class Escrows {{
  private readonly escrow: Contract = new Contract(A, ESCROW_ABI, P);
  #vault = new Contract(A, ESCROW_ABI, P);
  private token: Contract | null = null;
  constructor(private p: any) {{
    this.token = new Contract(A, TOKEN_ABI, p);
  }}
  early(id: string) {{
    return this.late.release(id);
  }}
  pay(id: string) {{
    this.escrow.deposit(id);
    this.#vault.release(id);
    this.token.decimals();
    escrow.release(id);
    vault.release(id);
  }}
  arm() {{
    this.late = new Contract(A, ESCROW_ABI, this.p);
    const signed = this.escrow.connect(this.p);
    return signed.release('x');
  }}
}}
class Other {{
  go(id: string) {{ return this.escrow.release(id); }}
}}
function loose(this: any, id: string) {{
  this.vault2 = new Contract(A, ESCROW_ABI, P);
  return this.vault2.release(id);
}}
class Params {{
  constructor(
    private held = new Contract(A, ESCROW_ABI, P),
    plain = new Contract(A, ESCROW_ABI, P),
  ) {{}}
  go(id: string, one = new Contract(A, ESCROW_ABI, P)) {{ this.held.deposit(id); this.one.release(id); return this.plain.release(id); }}
}}
class Inline {{
  constructor(public a: string, protected spare = new Contract(A, ESCROW_ABI, P)) {{}}
  go() {{ return this.spare.release('y'); }}
}}
"
        );
        let (rows, counts) = read(&src);
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=16",
                "call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=19",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=20",
                "call abi:TOKEN_ABI.decimals args=0 abi=f:decimals/0 line=21",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=28",
                "call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=43",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=47",
            ],
            "a `this.` binding reads in its whole class body (line 16 before its \
             assignment), never as a bare name, never in another class or outside one; \
             a parameter property is a field, a plain default parameter is none"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 2,
                not_in_abi: 0,
                dynamic: 0
            }
        );
    }

    #[test]
    fn static_call_and_v5_namespace_forms() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
async function preview(addr: string, p: any, id: string) {{
  const escrow = new Contract(addr, ESCROW_ABI, p);
  await escrow.release.staticCall(id);
  await escrow.deposit.send(id, {{ value: 1n }});
  await escrow.deposit.estimateGas(id);
  await escrow.release.populateTransaction(id);
  await escrow['release'].staticCallResult(id);
  await escrow.callStatic.release(id);
  await escrow.estimateGas.deposit(id, {{ value: 1n }});
  await escrow.populateTransaction.release(id);
  await escrow.functions.release(id);
  await escrow.callStatic.getAddress();
  await escrow.estimateGas.withdraw(id);
  await escrow.withdraw.staticCall(id);
  const fn = escrow.release.staticCall;
  await escrow.release.wait(id);
  await escrow.estimateGas(id);
}}
"
        );
        let (rows, counts) = read(&src);
        let abi = "abi=f:deposit/1,f:release/1,e:Released/2";
        let want: Vec<String> = [
            ("release", 1, 9),
            ("deposit", 2, 10),
            ("deposit", 1, 11),
            ("release", 1, 12),
            ("release", 1, 13),
            ("release", 1, 14),
            ("deposit", 2, 15),
            ("release", 1, 16),
            ("release", 1, 17),
        ]
        .iter()
        .map(|(m, args, line)| format!("call abi:ESCROW_ABI.{m} args={args} {abi} line={line}"))
        .collect();
        assert_eq!(rows, want, "args is the final call's argument count");
        assert_eq!(counts.not_in_abi, 2, "withdraw twice");
        assert_eq!(counts.dynamic, 0);
    }

    #[test]
    fn query_filter_filters_and_add_listener_events() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
async function watch(addr: string, p: any, cb: any) {{
  const escrow = new Contract(addr, ESCROW_ABI, p);
  escrow.addListener('Released', cb);
  escrow.on(escrow.filters.Released(), cb);
  await escrow.queryFilter('Released(bytes32,address)', 0);
  await escrow.queryFilter(escrow.filters.Released(null), 0, 9);
  const topic = escrow.filters.Released(id);
  escrow.connect(p).once(escrow.filters['Released'](), cb);
  escrow.filters.Funded();
  escrow.queryFilter('Funded');
  escrow.off('Released', cb);
  escrow.removeListener('Released', cb);
  escrow.queryFilter(filter, 0);
  const f = escrow.filters.Released;
}}
"
        );
        let (rows, counts) = read(&src);
        let want: Vec<String> = (9..=14)
            .map(|line| {
                format!(
                    "event abi:ESCROW_ABI.Released args=- abi=f:deposit/1,f:release/1,e:Released/2 line={line}"
                )
            })
            .collect();
        assert_eq!(
            rows, want,
            "a filter inside on / once / queryFilter is one site"
        );
        assert_eq!(counts.not_in_abi, 2, "Funded twice");
    }

    #[test]
    fn kina_wallet_service_shape() {
        // Kina frontend/src/app/core/services/wallet.service.ts, condensed.
        let escrow_fns: [(&str, &str, &str); 10] = [
            (
                "createTrade",
                "address buyer, uint256 amount",
                "buyer, amountBN",
            ),
            ("confirmPayment", "uint256 tradeId", "id"),
            ("releaseFunds", "uint256 tradeId", "id"),
            ("raiseDispute", "uint256 tradeId", "id"),
            ("cancelTrade", "uint256 tradeId", "id"),
            ("claimTimeout", "uint256 tradeId", "id"),
            ("claimTimeoutForBuyer", "uint256 tradeId", "id"),
            (
                "proposeResolution",
                "uint256 tradeId, uint8 outcome",
                "id, outcome",
            ),
            (
                "acceptResolution",
                "uint256 tradeId, uint8 expectedOutcome",
                "id, outcome",
            ),
            ("enterPenaltyHold", "uint256 tradeId", "id"),
        ];
        let mut src = String::from(
            "import { Injectable, inject, signal } from '@angular/core';
import { BrowserProvider, Contract, ethers, formatUnits } from 'ethers';

// Minimal ERC-20 ABI - only what we need.
const ERC20_ABI = [
  'function balanceOf(address) view returns (uint256)',
  'function decimals() view returns (uint8)',
  'function approve(address spender, uint256 amount) returns (bool)',
];

const ESCROW_ABI = [
",
        );
        for (f, params, _) in escrow_fns {
            src.push_str(&format!("  'function {f}({params})',\n"));
        }
        src.push_str(
            "];

@Injectable({ providedIn: 'root' })
export class WalletService {
  private provider: BrowserProvider | null = null;

  async approveEscrow(chain: Chain, amount: string): Promise<string> {
    const signer = await this.getSigner();
    const token = new Contract(this.getStablecoinAddress(chain), ERC20_ABI, signer);
    const tx = await token['approve'](environment.chains[chain].contractAddress, BigInt(amount));
    await tx.wait();
    return tx.hash;
  }
",
        );
        for (f, _, args) in escrow_fns {
            src.push_str(&format!(
                "
  async {f}(chain: Chain, id: string, buyer: string, amountBN: bigint, outcome: number): Promise<string> {{
    const signer = await this.getSigner();
    const escrow = new Contract(environment.chains[chain].contractAddress, ESCROW_ABI, signer);
    const tx = await escrow['{f}']({args});
    await tx.wait();
    return tx.hash;
  }}
"
            ));
        }
        src.push_str(
            "
  async loadBalance(chain: Chain): Promise<void> {
    if (!this.provider || !this._state().address) return;
    const token = new Contract(this.getStablecoinAddress(chain), ERC20_ABI, this.provider);
    const decimals = await token['decimals']();
    const raw = await token['balanceOf'](this._state().address);
    this._state.update(s => ({ ...s, balance: formatUnits(raw, decimals) }));
  }
}
",
        );
        let out = crate::evm::extract_evm_clients(
            &src,
            "angular",
            glia_core::NodeId(1),
            glia_core::RepoId(1),
        );
        let path = "frontend/src/app/core/services/wallet.service.ts";
        assert_eq!(
            out.markers(path),
            [format!(
                "[evm-client] lang=ts calls=13 events=0 sites=13 (ethers=13 hardhat=0 typechain=0 viem=0 web3=0 geth=0) abis=2 not_in_abi=0 dynamic=0 path={path}"
            )]
        );
        let mut qnames: Vec<&str> = out.nav.qname_by_id.values().map(String::as_str).collect();
        qnames.sort_unstable();
        let mut want: Vec<String> = escrow_fns
            .iter()
            .map(|(f, _, _)| format!("evm_call:abi:ESCROW_ABI.{f}"))
            .chain(
                ["approve", "balanceOf", "decimals"]
                    .iter()
                    .map(|f| format!("evm_call:abi:ERC20_ABI.{f}")),
            )
            .collect();
        want.sort_unstable();
        assert_eq!(
            qnames, want,
            "ten escrow['..'] sites over a 10-entry ABI, three token['..']"
        );
        let (rows, _) = read(&src);
        assert!(
            rows.iter().any(|r| r.starts_with(
                "call abi:ERC20_ABI.decimals args=0 abi=f:approve/2,f:balanceOf/1,f:decimals/0 "
            )),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.starts_with("call abi:ESCROW_ABI.createTrade args=2 ")),
            "{rows:?}"
        );
    }

    #[test]
    fn hardhat_factory_deploy_and_connect_chain() {
        // Kina contracts/test/EscrowManager.test.js, condensed: `let escrow;`
        // at describe level, the assignment in beforeEach.
        let src = r#"const { expect } = require("chai");
const { ethers } = require("hardhat");

describe("EscrowManager", function () {
  let escrow;
  let token;

  beforeEach(async function () {
    const MockToken = await ethers.getContractFactory("MockERC20");
    token = await MockToken.deploy("Mock USDT", "mUSDT", 6);
    await token.mint(seller.address, 1);
    const EscrowManager = await ethers.getContractFactory("EscrowManager");
    escrow = await EscrowManager.deploy(
      await token.getAddress(),
      platform.address,
    );
    await token.connect(seller).approve(await escrow.getAddress(), ethers.MaxUint256);
  });

  describe("Happy path", function () {
    it("creates a trade", async function () {
      const tx = await escrow.connect(seller).createTrade(buyer.address, AMOUNT);
      const trade = await escrow.getTrade(0);
      expect(await token.balanceOf(await escrow.getAddress())).to.equal(AMOUNT);
    });
  });
});
describe("Other", function () {
  it("reads nothing", async function () {
    await escrow.createTrade(a, b);
    await EscrowManager.deploy(1);
  });
});
"#;
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call MockERC20.mint args=2 abi=- line=11",
                "call MockERC20.approve args=2 abi=- line=17",
                "call EscrowManager.createTrade args=2 abi=- line=22",
                "call EscrowManager.getTrade args=1 abi=- line=23",
                "call MockERC20.balanceOf args=1 abi=- line=24",
            ],
            "a factory's deploy binds the describe-level `let`; the connect chain and \
             every it(..) block read it; the factory and getAddress are no site; \
             another describe sees neither"
        );
        assert_eq!(counts, TsCounts::default());
        assert!(libs(src).iter().all(|l| l.ends_with(" hardhat")));
    }

    #[test]
    fn get_contract_at_name_and_inline_abi() {
        // Kina contracts/scripts/provision_demo_wallets.js, condensed.
        let src = format!(
            "const hre = require(\"hardhat\");\n{ABI}\
async function main() {{
  const [deployer] = await hre.ethers.getSigners();
  const audd = await hre.ethers.getContractAt(
    [\"function mint(address to, uint256 amount) external\",
     \"function balanceOf(address) view returns (uint256)\",
     \"function decimals() view returns (uint8)\"],
    deploy.stablecoin,
    deployer
  );
  const auddBal = await audd.balanceOf(w.address);
  const tx = await audd.mint(w.address, need, {{ nonce: nonce++ }});
  await audd.transfer(w.address, 1);
  const escrow = await hre.ethers.getContractAt('contracts/EscrowManager.sol:EscrowManager', deploy.address);
  await escrow.freeze(1);
  const viaConst = await ethers.getContractAt(ESCROW_ABI, deploy.address);
  await viaConst.release(id);
  const dyn = await ethers.getContractAt(loadAbi(), deploy.address);
  await dyn.release(id);
  await hre.ethers.getContractAt('EscrowManager', a).then((c) => c.freeze(2));
}}
"
        );
        let (rows, counts) = read(&src);
        let audd = "abi=f:balanceOf/1,f:decimals/0,f:mint/2";
        assert_eq!(
            rows,
            [
                format!("call abi:audd.balanceOf args=1 {audd} line=16"),
                format!("call abi:audd.mint args=3 {audd} line=17"),
                "call EscrowManager.freeze args=1 abi=- line=20".to_string(),
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=22"
                    .to_string(),
            ],
            "a name (a qualified one too) is the contract; an inline ABI scopes by \
             its variable; an ABI const reads like a `new Contract` argument"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 2,
                not_in_abi: 1,
                dynamic: 1
            },
            "the inline literal counts as an ABI; transfer is not in it; loadAbi() is dynamic"
        );
        assert!(libs(&src).iter().all(|l| l.ends_with(" hardhat")));
    }

    #[test]
    fn deploy_contract_and_wait_for_deployment_is_stop_listed() {
        let src = "import { ethers } from 'hardhat';
describe('Token', () => {
  let token;
  beforeEach(async () => {
    token = await ethers.deployContract('Token', [1000n]);
    await token.waitForDeployment();
    await token.deploymentTransaction()?.wait();
  });
  it('mints', async () => {
    await token.mint(owner.address, 5);
    expect(await token.getAddress()).to.properAddress;
    await token.deployed();
    console.log(token.target, token.interface);
  });
});
const v = await hre.viem.deployContract('Token');
await v.write.mint([a, 1n]);
const loose = ethers.deployContract('Token').mint(1);
";
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call Token.mint args=2 abi=- line=10",
                "call Token.mint args=2 abi=- line=17"
            ],
            "ethers runtime members are no site; viem's deployContract is no hardhat-ethers \
             helper but hardhat-viem's (DT.10j), read in viem; a member call on the promise \
             is not read"
        );
        assert_eq!(counts, TsCounts::default());
        assert_eq!(libs(src), ["Token.mint hardhat", "Token.mint viem"]);
    }

    #[test]
    fn typechain_connect_chained_and_bound() {
        let src = "import { Escrow__factory, Token__factory } from '../typechain-types';
import * as types from '../typechain-types';
import { Contract } from 'ethers';

export async function run(addr: string, signer: Signer, other: Signer) {
  await Escrow__factory.connect(addr, signer).release(id);
  const escrow = Escrow__factory.connect(addr, signer);
  await escrow.deposit(id, { value: 1 });
  await escrow.connect(other).release(id);
  const factory = new Token__factory(signer);
  const token = await factory.deploy(1000n);
  await token.mint(a, 1);
  const attached = new Token__factory(signer).attach(addr);
  await attached.balanceOf(a);
  await new Token__factory(signer).attach(addr).connect(other).mint(a, 2);
  await types.Escrow__factory.connect(addr, signer).deposits(id);
  const iface = Escrow__factory.createInterface();
  const abiOnly = new Contract(addr, Escrow__factory.abi, signer);
  await abiOnly.release(id);
  await escrow.getAddress();
}
";
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call Escrow.release args=1 abi=- line=6",
                "call Escrow.deposit args=2 abi=- line=8",
                "call Escrow.release args=1 abi=- line=9",
                "call Token.mint args=2 abi=- line=12",
                "call Token.balanceOf args=1 abi=- line=14",
                "call Token.mint args=2 abi=- line=15",
                "call Escrow.deposits args=1 abi=- line=16",
                "call Escrow.release args=1 abi=- line=19",
            ]
        );
        assert_eq!(counts, TsCounts::default());
        let libs = libs(src);
        assert_eq!(libs.iter().filter(|l| l.ends_with(" typechain")).count(), 7);
        assert_eq!(
            libs.last().map(String::as_str),
            Some("Escrow.release ethers"),
            "`new Contract(.., Escrow__factory.abi, ..)` is an ethers construction"
        );
        // The gate: a TypeChain factory identifier is enough on its own.
        let bare = "const e = Escrow__factory.connect(a, s);\ne.release(1);\n";
        assert_eq!(read(bare).0, ["call Escrow.release args=1 abi=- line=2"]);
        let path = "import { Escrow } from './typechain/contracts';\nconst c = new Contract(a, ABI, s);\nc.go();\n";
        assert_eq!(read(path).0, ["call abi:ABI.go args=0 abi=- line=3"]);
    }

    #[test]
    fn artifact_import_names_the_contract_other_json_does_not() {
        let src = "import { Contract } from 'ethers';
import { ethers } from 'hardhat';
import EscrowArtifact from '../artifacts/contracts/Escrow.sol/Escrow.json';
import { abi as tokenAbi } from '../out/Token.sol/Token.json';
import * as Vault from '../artifacts/contracts/vault/Vaults.sol/Vault.json';
import settings from './settings.json';
import EscrowDbg from '../artifacts/contracts/Escrow.sol/Escrow.dbg.json';
const Pool = require('../artifacts/contracts/Pool.sol/Pool.json');
const { abi: lendAbi } = require('../out/Lend.sol/Lend.json');
const bondAbi = require('../out/Bond.sol/Bond.json').abi;
import Vote = require('../out/Gov.sol/Vote.json');

export async function wire(a: string, s: any) {
  new Contract(a, EscrowArtifact.abi, s).release(id);
  const token = new Contract(a, tokenAbi, s);
  token.mint(a, 1);
  const vault = new Contract(a, Vault.abi, s);
  vault.sweep();
  const pool = new Contract(a, Pool.abi, s);
  pool.swap(1);
  const lend = new Contract(a, lendAbi, s);
  lend.borrow(1);
  const bond = new Contract(a, bondAbi, s);
  bond.redeem();
  const conf = new Contract(a, settings.abi, s);
  conf.apply(1);
  const dbg = new Contract(a, EscrowDbg.abi, s);
  dbg.release(1);
  const other = new Contract(a, config.abi, s);
  other.go();
  const viaAt = await ethers.getContractAt(EscrowArtifact.abi, a);
  await viaAt.deposit(id);
  const vote = new Contract(a, Vote, s);
  vote.cast(1);
}
";
        let view = code_view(src, "typescript");
        let imports = json_imports(src, &view, &declarations(&view));
        let pairs: Vec<(&str, Option<&str>)> = imports
            .iter()
            .map(|i| (i.name.as_str(), i.contract.as_deref()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("EscrowArtifact", Some("Escrow")),
                ("tokenAbi", Some("Token")),
                ("Vault", Some("Vault")),
                ("settings", None),
                ("EscrowDbg", None),
                ("Pool", Some("Pool")),
                ("lendAbi", Some("Lend")),
                ("bondAbi", Some("Bond")),
                ("Vote", Some("Vote")),
            ]
        );
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call Escrow.release args=1 abi=- line=14",
                "call Token.mint args=2 abi=- line=16",
                "call Vault.sweep args=0 abi=- line=18",
                "call Pool.swap args=1 abi=- line=20",
                "call Lend.borrow args=1 abi=- line=22",
                "call Bond.redeem args=0 abi=- line=24",
                "call abi:settings.apply args=1 abi=- line=26",
                "call abi:EscrowDbg.release args=1 abi=- line=28",
                "call Escrow.deposit args=1 abi=- line=32",
                "call Vote.cast args=1 abi=- line=34",
            ],
            "an artifact (hardhat or foundry layout) names its contract; any other JSON \
             import is an ABI of unknown entries; a member of no import is dynamic"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 0,
                not_in_abi: 0,
                dynamic: 1
            }
        );
        assert_eq!(
            libs(src).iter().rev().nth(1).map(String::as_str),
            Some("Escrow.deposit hardhat")
        );
    }

    #[test]
    fn chai_to_emit_is_not_a_subscription() {
        let src = "const { expect } = require('chai');
const { ethers } = require('hardhat');
it('emits', async () => {
  const escrow = await ethers.deployContract('Escrow');
  await expect(escrow.release(id)).to.emit(escrow, 'Released').withArgs(id);
  await expect(tx).to.be.revertedWithCustomError(escrow, 'NotSeller');
  await expect(escrow.connect(s).release(id)).to.changeTokenBalances(token, [a], [1]);
  escrow.on('Released', cb);
  const art = await hre.artifacts.readArtifact('Escrow');
  const c = new ethers.Contract(a, art.abi, s);
  c.release(1);
});
";
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call Escrow.release args=1 abi=- line=5",
                "call Escrow.release args=1 abi=- line=7",
                "event Escrow.Released args=- abi=- line=8",
            ],
            "a chai matcher takes the contract as an argument: no subscription; a \
             readArtifact ABI is not read"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 0,
                not_in_abi: 0,
                dynamic: 1
            }
        );
    }

    #[test]
    fn upgrades_proxies_and_factory_links() {
        let src = "const { ethers, upgrades } = require('hardhat');
async function main() {
  const Box = await ethers.getContractFactory('contracts/Box.sol:Box');
  const box = await upgrades.deployProxy(Box, [42], { initializer: 'store' });
  await box.store(7);
  const BoxV2 = await hre.ethers.getContractFactory('BoxV2', signer);
  const upgraded = await hre.upgrades.upgradeProxy(await box.getAddress(), BoxV2);
  await upgraded.increment();
  const again = await Box.connect(admin).deploy();
  await again.retrieve();
  const at = Box.attach(addr);
  await at.store(1);
  await Box.attach(addr).retrieve();
  await (await Box.deploy()).store(2);
  const fromInline = await (await ethers.getContractFactory('Box')).deploy();
  await fromInline.store(3);
  const stray = await upgrades.deployProxy(Unbound, []);
  await stray.store(4);
}
";
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call Box.store args=1 abi=- line=5",
                "call BoxV2.increment args=0 abi=- line=8",
                "call Box.retrieve args=0 abi=- line=10",
                "call Box.store args=1 abi=- line=12",
                "call Box.retrieve args=0 abi=- line=13",
            ],
            "deployProxy / upgradeProxy read their factory argument; a factory's \
             connect link keeps it a factory; an inline promise and an unbound factory \
             are not read"
        );
        assert_eq!(counts, TsCounts::default());
    }

    #[test]
    fn fixture_files_name_the_contract() {
        // bench/substrate-gap/fixtures/ts-hardhat-contract-calls/contracts/.
        let test_js = include_str!(
            "../../../../../bench/substrate-gap/fixtures/ts-hardhat-contract-calls/contracts/test/Escrow.test.js"
        );
        let settle = include_str!(
            "../../../../../bench/substrate-gap/fixtures/ts-hardhat-contract-calls/contracts/scripts/settle.ts"
        );
        let extract = |src: &str| {
            crate::evm::extract_evm_clients(
                src,
                "typescript",
                glia_core::NodeId(1),
                glia_core::RepoId(1),
            )
        };
        let qnames = |out: &crate::evm::EvmNodes| {
            let mut q: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
            q.sort_unstable();
            q
        };
        let out = extract(test_js);
        assert_eq!(
            out.markers("test/Escrow.test.js"),
            [
                "[evm-client] lang=ts calls=5 events=0 sites=5 (ethers=0 hardhat=5 typechain=0 viem=0 web3=0 geth=0) abis=0 not_in_abi=0 dynamic=0 path=test/Escrow.test.js"
            ]
        );
        assert_eq!(
            qnames(&out),
            [
                "evm_call:Escrow.deposit",
                "evm_call:Escrow.deposits",
                "evm_call:Escrow.release",
                "evm_call:Token.approve",
                "evm_call:Token.mint",
            ]
        );
        let out = extract(settle);
        assert_eq!(
            out.markers("scripts/settle.ts"),
            [
                "[evm-client] lang=ts calls=4 events=0 sites=5 (ethers=1 hardhat=3 typechain=1 viem=0 web3=0 geth=0) abis=1 not_in_abi=0 dynamic=0 path=scripts/settle.ts"
            ]
        );
        assert_eq!(
            qnames(&out),
            [
                "evm_call:Escrow.deposits",
                "evm_call:Escrow.release",
                "evm_call:abi:token.balanceOf",
                "evm_call:abi:token.mint",
            ]
        );
        let cells_of = |qname: &str| -> Vec<ClientFacts> {
            out.nodes
                .iter()
                .filter(|n| out.nav.qname_by_id[&n.id] == qname)
                .flat_map(|n| read_client_cells(&n.cells))
                .collect()
        };
        let facts = |lib: &str, args: Vec<u32>, abi: Option<Vec<&str>>| ClientFacts {
            lib: lib.to_string(),
            args,
            abi: abi.map(|a| a.into_iter().map(str::to_string).collect()),
        };
        assert_eq!(
            cells_of("evm_call:Escrow.release"),
            [
                facts("ethers", vec![1], None),
                facts("typechain", vec![1], None)
            ],
            "the artifact construction and the TypeChain connect: one cell per library"
        );
        assert_eq!(
            cells_of("evm_call:abi:token.mint"),
            [facts(
                "hardhat",
                vec![2],
                Some(vec!["f:balanceOf/1", "f:mint/2"])
            )]
        );
    }
    #[test]
    fn viem_write_read_simulate_with_function_name_and_args() {
        let src = "import { parseAbi } from 'viem';
import { readContract } from 'viem/actions';
const escrowAbi = parseAbi([
  'function deposit(bytes32 orderId) payable',
  'function release(bytes32 orderId)',
  'function deposits(bytes32) view returns (uint256)',
  'function sweep()',
]);
export async function run(client: any, wallet: any, address: `0x${string}`, id: `0x${string}`, args: any) {
  await wallet.writeContract({ address, abi: escrowAbi, functionName: 'deposit', args: [id], value: 1n });
  await readContract(client, { address, abi: escrowAbi, functionName: 'deposits', args: [id] });
  const { request } = await client.simulateContract<typeof escrowAbi, 'release'>({
    address,
    abi: escrowAbi as Abi,
    functionName: 'release' as const,
    args: [id] as const,
  });
  await wallet.writeContract(request);
  await client.estimateContractGas({ address, abi: escrowAbi, functionName: 'sweep' });
  await client.readContract({ address, abi: escrowAbi, functionName: 'withdraw', args: [id] });
  await client.readContract({ address, abi: escrowAbi, functionName: 'deposits', args });
  const c = new Contract(address, escrowAbi, wallet);
  await c.release(id);
}
";
        let (rows, counts) = read(src);
        let abi = "abi=f:deposit/1,f:deposits/1,f:release/1,f:sweep/0";
        assert_eq!(
            rows,
            [
                format!("call abi:escrowAbi.deposit args=1 {abi} line=10"),
                format!("call abi:escrowAbi.deposits args=1 {abi} line=11"),
                format!("call abi:escrowAbi.release args=1 {abi} line=15"),
                format!("call abi:escrowAbi.sweep args=0 {abi} line=19"),
                format!("call abi:escrowAbi.deposits args=- {abi} line=21"),
            ],
            "every option-call shape; a type argument list, `as` casts and an absent `args` \
             (0) read; a request object names nothing; a viem-only file reads no `new Contract`"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 1,
                dynamic: 0
            },
            "parseAbi binds the ABI; withdraw is not in it"
        );
        assert!(libs(src).iter().all(|l| l.ends_with(" viem")));
    }

    #[test]
    fn viem_multicall_contracts_each() {
        let src = "import { multicall } from 'viem/actions';
import { useReadContracts } from 'wagmi';
import { escrowAbi } from './abis';
const vaultAbi = [
  { type: 'function', name: 'balance', inputs: [{ name: 'who', type: 'address' }] },
  { type: 'function', name: 'total', inputs: [] },
] as const;
export async function totals(client: any, escrow: `0x${string}`, vault: `0x${string}`, id: string) {
  return multicall(client, {
    contracts: [
      { address: escrow, abi: escrowAbi, functionName: 'deposits', args: [id] },
      { address: vault, abi: vaultAbi, functionName: 'balance', args: [escrow] },
      { address: vault, abi: vaultAbi, functionName: 'total' },
      someCall,
    ],
    allowFailure: false,
  });
}
export function useTotals(vault: `0x${string}`) {
  return useReadContracts({ contracts: [{ address: vault, abi: vaultAbi, functionName: 'total' }, { ...vaultContract, functionName: 'total' }] });
}
export const loose = (client: any, calls: any[]) => multicall(client, { contracts: calls });
";
        let (rows, counts) = read(src);
        let vault = "abi=f:balance/1,f:total/0";
        assert_eq!(
            rows,
            [
                "call abi:escrowAbi.deposits args=1 abi=- line=11".to_string(),
                format!("call abi:vaultAbi.balance args=1 {vault} line=12"),
                format!("call abi:vaultAbi.total args=0 {vault} line=13"),
                format!("call abi:vaultAbi.total args=0 {vault} line=20"),
            ],
            "each object element of `contracts` is one call; an imported ABI has no entries"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 0,
                dynamic: 3
            },
            "an element that is no object, a spread with no abi, a `contracts` that is no literal"
        );
    }

    #[test]
    fn viem_get_contract_read_write_watch_event() {
        let src = "import { getContract, parseAbi } from 'viem';
const escrowAbi = parseAbi([
  'function deposit(bytes32 orderId) payable',
  'function release(bytes32 orderId)',
  'function deposits(bytes32) view returns (uint256)',
  'event Released(bytes32 indexed orderId, address seller)',
]);
export class EscrowClient {
  private contract = getContract({ address: ESCROW_ADDRESS, abi: escrowAbi, client: { public: pub, wallet } });
  async pay(id: `0x${string}`) {
    await this.contract.write.deposit([id], { value: 1n });
    await this.contract.simulate.release([id]);
    await this.contract.estimateGas.release([id], { account });
    this.contract.watchEvent.Released({ orderId: id }, { onLogs: () => {} });
    await this.contract.getEvents.Released();
    console.log(this.contract.address, this.contract.abi);
    await this.contract.read.withdraw();
    await this.contract.read();
    await this.contract.write.deposit({ value: 1n });
  }
}
export const held = (id: `0x${string}`) =>
  getContract({ address: ESCROW_ADDRESS, abi: escrowAbi, client }).read.deposits([id]);
const inline = getContract({ address: a, abi: parseAbi(['function ping()']), client });
inline.write.ping();
const lost = getContract({ address: a, abi: loadAbi(), client });
lost.read.deposits([id]);
";
        let (rows, counts) = read(src);
        let abi = "abi=f:deposit/1,f:deposits/1,f:release/1,e:Released/2";
        assert_eq!(
            rows,
            [
                format!("call abi:escrowAbi.deposit args=1 {abi} line=11"),
                format!("call abi:escrowAbi.release args=1 {abi} line=12"),
                format!("call abi:escrowAbi.release args=1 {abi} line=13"),
                format!("event abi:escrowAbi.Released args=- {abi} line=14"),
                format!("event abi:escrowAbi.Released args=- {abi} line=15"),
                format!("call abi:escrowAbi.deposit args=0 {abi} line=19"),
                format!("call abi:escrowAbi.deposits args=1 {abi} line=23"),
                "call abi:inline.ping args=0 abi=f:ping/0 line=25".to_string(),
            ],
            "a class field instance, a chained instance and an inline-ABI instance; no \
             namespace is a member"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 2,
                not_in_abi: 1,
                dynamic: 1
            },
            "parseAbi bound and inline; withdraw is not in the ABI; loadAbi() is dynamic"
        );
    }

    #[test]
    fn hardhat_viem_deploy_contract_names_the_contract() {
        let src = "import hre from 'hardhat';
import { viem } from 'hardhat';
describe('Escrow', () => {
  let escrow;
  beforeEach(async () => {
    escrow = await hre.viem.deployContract('Escrow', [owner]);
  });
  it('deposits', async () => {
    await escrow.write.deposit([id], { value: 1n });
    expect(await escrow.read.deposits([id])).to.equal(1n);
  });
});
async function attach(addr: `0x${string}`) {
  const at = await viem.getContractAt('contracts/Vault.sol:Vault', addr);
  await at.write.release([id]);
  const viaEthers = await hre.ethers.getContractAt('Vault', addr);
  await viaEthers.release(id);
  const nope = await hre.viem.getContractAt(addr);
  nope.write.release([id]);
  const client = await hre.viem.getPublicClient();
  await client.readContract({ address: addr, abi: vaultAbi, functionName: 'total' });
}
";
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call Escrow.deposit args=1 abi=- line=9",
                "call Escrow.deposits args=1 abi=- line=10",
                "call Vault.release args=1 abi=- line=15",
                "call Vault.release args=1 abi=- line=17",
                "call abi:vaultAbi.total args=0 abi=- line=21",
            ],
            "deployContract / getContractAt on (hre.)viem name the contract; a hardhat \
             import opens both readers; a getContractAt with no name binds nothing"
        );
        assert_eq!(counts, TsCounts::default());
        assert_eq!(
            libs(src),
            [
                "Escrow.deposit viem",
                "Escrow.deposits viem",
                "Vault.release viem",
                "Vault.release hardhat",
                "abi:vaultAbi.total viem",
            ]
        );
    }

    #[test]
    fn wagmi_use_read_contract_and_use_watch_contract_event() {
        let src = "import { useReadContract, useWatchContractEvent, useWriteContract, useSimulateContract, useContractEvent } from 'wagmi';
import { parseEventLogs } from 'viem';
import { escrowAbi } from '../generated';
export function Held({ id }: { id: `0x${string}` }) {
  const { data } = useReadContract({ address: ESCROW_ADDRESS, abi: escrowAbi, functionName: 'deposits', args: [id] });
  useWatchContractEvent({
    address: ESCROW_ADDRESS,
    abi: escrowAbi,
    eventName: 'Released',
    onLogs(logs) { console.log(logs); },
  });
  useWatchContractEvent({ address: ESCROW_ADDRESS, abi: escrowAbi, onLogs });
  const { writeContract } = useWriteContract();
  const release = () => writeContract({ address: ESCROW_ADDRESS, abi: escrowAbi, functionName: 'release', args: [id] });
  const { data: sim } = useSimulateContract({ address: ESCROW_ADDRESS, abi: escrowAbi, functionName: 'release', args: [id] });
  useContractEvent({ address: ESCROW_ADDRESS, abi: escrowAbi, eventName: 'Funded', listener });
  const logs = parseEventLogs({ abi: escrowAbi, eventName: ['Released', 'Funded'], logs: raw });
  return <button onClick={release}>{String(data)}</button>;
}
";
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call abi:escrowAbi.deposits args=1 abi=- line=5",
                "event abi:escrowAbi.Released args=- abi=- line=9",
                "call abi:escrowAbi.release args=1 abi=- line=14",
                "call abi:escrowAbi.release args=1 abi=- line=15",
                "event abi:escrowAbi.Funded args=- abi=- line=16",
                "event abi:escrowAbi.Released args=- abi=- line=17",
                "event abi:escrowAbi.Funded args=- abi=- line=17",
            ],
            "wagmi hooks read like viem actions; a watch with no eventName names nothing; \
             parseEventLogs reads each name of its array"
        );
        assert_eq!(counts, TsCounts::default());
    }

    #[test]
    fn web3_methods_and_events() {
        let src = "const Web3 = require('web3');
const ESCROW_JSON = [
  { type: 'function', name: 'release', inputs: [{ name: 'orderId', type: 'bytes32' }] },
  { type: 'function', name: 'connect', inputs: [] },
  { type: 'event', name: 'Released', inputs: [{ name: 'orderId', type: 'bytes32', indexed: true }] },
];
async function run(web3, address, id, from) {
  const escrow = new web3.eth.Contract(ESCROW_JSON, address);
  await escrow.methods.release(id).send({ from });
  await escrow.methods['release(bytes32)'](id).call();
  await escrow.methods.connect().call();
  await escrow.methods.withdraw(id).estimateGas();
  escrow.events.Released({ filter: { orderId: id } }, cb);
  escrow.events.allEvents({}, cb);
  await escrow.getPastEvents('Released', { fromBlock: 0 });
  escrow.once('Released', cb);
  escrow.getPastEvents('allEvents');
  console.log(escrow.options.address);
  await new web3.eth.Contract(ESCROW_JSON, address).methods.release(id).call();
  const token = new this.web3.eth.Contract(TOKEN_ABI);
  token.methods.transfer(a, 1).send();
  const bad = new web3.eth.Contract(loadAbi(), address);
  bad.methods.release(id);
  const viaEthers = new Contract(address, ESCROW_JSON, signer);
  viaEthers.release(id);
}
";
        let (rows, counts) = read(src);
        let abi = "abi=f:connect/0,f:release/1,e:Released/1";
        assert_eq!(
            rows,
            [
                format!("call abi:ESCROW_JSON.release args=1 {abi} line=9"),
                format!("call abi:ESCROW_JSON.release args=1 {abi} line=10"),
                format!("call abi:ESCROW_JSON.connect args=0 {abi} line=11"),
                format!("event abi:ESCROW_JSON.Released args=- {abi} line=13"),
                format!("event abi:ESCROW_JSON.Released args=- {abi} line=15"),
                format!("event abi:ESCROW_JSON.Released args=- {abi} line=16"),
                format!("call abi:ESCROW_JSON.release args=1 {abi} line=19"),
                "call abi:TOKEN_ABI.transfer args=2 abi=- line=21".to_string(),
            ],
            "methods / events / getPastEvents / once; send / call / estimateGas are no \
             member; no ethers stop list; a web3-only file reads no `new Contract`"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 1,
                dynamic: 1
            },
            "the object-literal ABI; withdraw is not in it; loadAbi() is dynamic"
        );
        assert!(libs(src).iter().all(|l| l.ends_with(" web3")));
        let ungated = "async function run(web3, a) {\n  const e = new web3.eth.Contract(ABI, a);\n  e.methods.release(1).send();\n}\n";
        assert_eq!(
            read(ungated),
            (Vec::new(), TsCounts::default()),
            "no web3 import"
        );
    }

    #[test]
    fn dynamic_function_name_counts_dynamic() {
        let src = "import { readContract } from '@wagmi/core';
import { parseEventLogs } from 'viem';
import { escrowAbi } from './abi';
export async function call(config: Config, fn: string, events: string[], opts: any) {
  await readContract(config, { address: ESCROW_ADDRESS, abi: escrowAbi, functionName: fn, args: [] });
  await readContract(config, { address: ESCROW_ADDRESS, abi: escrowAbi, functionName: `deposits`, args: [] });
  await readContract(config, { address: ESCROW_ADDRESS, abi: loadAbi(), functionName: 'deposits' });
  await readContract(config, { address: ESCROW_ADDRESS, abi: escrowAbi, functionName: `get${name}` });
  await readContract(config, { ...base, functionName: 'deposits' });
  await readContract(config, opts);
  parseEventLogs({ abi: escrowAbi, eventName: events, logs });
  parseEventLogs({ abi: escrowAbi, logs });
}
class Reader {
  readContract({ abi, functionName }: Opts) { return functionName; }
  simulateContract({ abi, functionName }: Opts): Promise<Result> { return run(abi, functionName); }
}
function writeContract({ abi, functionName }: Opts) { return functionName; }
";
        let (rows, counts) = read(src);
        assert_eq!(rows, ["call abi:escrowAbi.deposits args=0 abi=- line=6"]);
        assert_eq!(
            counts,
            TsCounts {
                abis: 0,
                not_in_abi: 0,
                dynamic: 5
            },
            "a variable name, an unreadable ABI, a template with ${{..}}, a spread with no \
             abi and an eventName variable; options in a variable and definitions are no call"
        );
    }

    #[test]
    fn shorthand_abi_falls_back_to_the_address_hint() {
        let src = "import { readContract, writeContract } from 'viem/actions';
export async function viaParams(client: any, abi: Abi, id: string, address: `0x${string}`) {
  await readContract(client, { address: ESCROW_ADDRESS, abi, functionName: 'deposits', args: [id] });
  await writeContract(client, { address: escrowAddr as `0x${string}`, abi: abi, functionName: 'release', args: [id] });
  await readContract(client, { address, abi, functionName: 'deposits', args: [id] });
  await readContract(client, { address: VAULT_ADDRESS, abi: ['function ping()'], functionName: 'ping' });
  await readContract(client, { address: '0x01', abi: parseAbi(['function ping()']), functionName: 'ping' });
  await readContract(client, { address: config.escrow, abi, functionName: 'deposits' });
}
";
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ADDRESS.deposits args=1 abi=- line=3",
                "call abi:escrowAddr.release args=1 abi=- line=4",
                "call abi:VAULT_ADDRESS.ping args=0 abi=f:ping/0 line=6",
            ],
            "an `abi` the file binds nowhere, or an inline ABI, scopes by the address \
             identifier when it names a contract"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 0,
                dynamic: 3
            },
            "a bare `address`, a literal address and a member path name nothing"
        );
        let bound = "import { readContract } from 'viem/actions';
import { abi } from './generated/Escrow';
readContract(client, { address: ESCROW_ADDRESS, abi, functionName: 'release', args: [id] });
";
        assert_eq!(
            read(bound).0,
            ["call abi:abi.release args=1 abi=- line=3"],
            "an imported `abi` is the binding, not the hint"
        );
        let local = "import { parseAbi, readContract } from 'viem';
const abi = parseAbi(['function release(bytes32)']);
readContract(client, { address: ESCROW_ADDRESS, abi, functionName: 'release', args: [id] });
";
        assert_eq!(
            read(local).0,
            ["call abi:abi.release args=1 abi=f:release/1 line=3"]
        );
    }

    #[test]
    fn fixture_files_read_viem_wagmi_and_web3() {
        // bench/substrate-gap/fixtures/ts-viem-web3-contract-calls/.
        let fx = |path: &str| -> (String, Vec<String>) {
            let root = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../bench/substrate-gap/fixtures/ts-viem-web3-contract-calls/"
            );
            let src = std::fs::read_to_string(format!("{root}{path}")).expect("fixture file");
            let out = crate::evm::extract_evm_clients(
                &src,
                "typescript",
                glia_core::NodeId(1),
                glia_core::RepoId(1),
            );
            let short = path.split_once('/').map_or(path, |(_, rest)| rest);
            let mut lines = out.markers(short);
            let mut q: Vec<String> = out.nav.qname_by_id.values().cloned().collect();
            q.sort_unstable();
            lines.extend(q);
            (src, lines)
        };
        let line = |c: &str, l: &str, a: usize, p: &str| {
            format!(
                "[evm-client] lang=ts {c} (ethers=0 hardhat=0 typechain=0 {l} geth=0) abis={a} \
                 not_in_abi=0 dynamic=0 path={p}"
            )
        };
        assert_eq!(
            fx("contracts/test/Escrow.viem.ts").1,
            [
                line(
                    "calls=2 events=0 sites=2",
                    "viem=2 web3=0",
                    0,
                    "test/Escrow.viem.ts"
                ),
                "evm_call:Escrow.deposit".to_string(),
                "evm_call:Escrow.release".to_string(),
            ]
        );
        assert_eq!(
            fx("dapp/src/escrow.ts").1,
            [
                line(
                    "calls=2 events=1 sites=4",
                    "viem=4 web3=0",
                    1,
                    "src/escrow.ts"
                ),
                "evm_call:abi:escrowAbi.deposits".to_string(),
                "evm_call:abi:escrowAbi.release".to_string(),
                "evm_event:abi:escrowAbi.Released".to_string(),
            ]
        );
        assert_eq!(
            fx("dapp/src/legacy.js").1,
            [
                line(
                    "calls=1 events=1 sites=2",
                    "viem=0 web3=2",
                    1,
                    "src/legacy.js"
                ),
                "evm_call:abi:ESCROW_JSON.release".to_string(),
                "evm_event:abi:ESCROW_JSON.Released".to_string(),
            ]
        );
        assert_eq!(
            fx("dapp/src/vault.ts").1,
            [
                line(
                    "calls=1 events=0 sites=1",
                    "viem=1 web3=0",
                    1,
                    "src/vault.ts"
                ),
                "evm_call:abi:vaultAbi.withdraw".to_string(),
            ]
        );
    }
}
