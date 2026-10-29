"""The evm transport (FH.11c): a Solidity contract, `Escrow`, served on-chain to a TypeScript client
(ethers with a human-readable ABI, viem's readContract / writeContract with a JSON ABI, or a
TypeChain binding) or a Go client (go-ethereum's BoundContract over a JSON ABI, or an abigen v1
binding). James, 2026-10-25: "the best a/b is the contract definitions we found errant between
frontend backend and solidity".

The exchange: the client calls `deposit` (the pair run.py grades: RPC_CALLS from the client's call
to `evm:Escrow.deposit`), `release` and the public getter `arbiter`, and subscribes to the
`Released` event (EVENT_FLOWS from the contract's event to the client's subscription). At S the
contract drifts one way, chosen per case by `drift_of` (its cover number mod 3):

  fn_rename     `release` becomes `settle`
  event_rename  the event `Released` becomes `Settled`
  arity         `release` takes a third argument, `bytes32 memo`

and at C the client follows. The axes read here:

  contract       the client's ABI: `plain-literal` written inline (ethers' human-readable
                 fragments; a Go JSON ABI const in the calling file), `schema-object` a JSON ABI
                 imported from another module (viem's `as const` array; a Go const in another file
                 of the package, DT.10g), `generated` a typed binding (TypeChain; abigen v1)
  wiring_call    `direct` (the api layer calls the contract member by name), `wrapper` (a
                 hand-written method-name helper, `callEscrow('deposit', ..)` / Go
                 `c.submit(opts, "deposit", ..)`, DT.10f's shape), `generated-sdk` (the generated
                 binding is the api layer: the app calls a TypeChain instance the api layer
                 exports, or an abigen package of its own)
  wiring_source  the contract address: a `literal` at the use, or a `const`
  packaging      `single` (client/ and contracts/ in one repo), `workspace` (a pnpm or go.work
                 workspace: packages/api-client, apps/web, the contracts beside them),
                 `multi-repo` (a client repo and a contracts repo)

`expected(case)` names, besides run.py's pair, every edge the exchange should hold at P (`edges`)
and, per drift, the client files and lines contract-breaks / review should name at S
(`drift.client_sites`), all read off the renders."""

import json
import re
from dataclasses import dataclass, field

from . import rest_servers
from .common import fill

CATEGORY = "RPC_CALLS"
STATES = ("P", "S", "C")
CONTRACT = "Escrow"
PAIR_FN = "deposit"
GETTER = "arbiter"
# Well-known hardhat dev addresses: the first deployment, accounts #0 and #1.
ADDRESS = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
PAYER = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
PAYEE = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
DRIFTS = ("fn_rename", "event_rename", "arity")

# The client / server values these templates render (gen.py narrows the axes by them).
VALUES = {"client": ["ts", "go"], "server": ["solidity"]}
UNEXPRESSIBLE = [
    (("transport", "evm"), ("wiring_source", "enum-prefix"),
     "an EVM call names its contract by ABI and address; there is no path or channel segment "
     "to prefix"),
    (("transport", "evm"), ("contract", "declared-type"),
     "a contract's interface is its ABI: fragments, a JSON ABI or a generated binding, never a "
     "hand-declared payload type"),
]

VERSIONS = {"ethers": "^6.13.4", "viem": "^2.21.45", "hardhat": "^2.22.15",
            "typechain": "^8.3.2", "@typechain/ethers-v6": "^0.5.1",
            "go-ethereum": "v1.14.11"}
GETH = "github.com/ethereum/go-ethereum"


# ------------------------------------------------------------------------------- the contract

@dataclass(frozen=True)
class Spec:
    """One side's view of the contract: the drifting function's name, the drifting event's name
    and whether `release` takes the memo."""

    release: str = "release"
    event: str = "Released"
    memo: bool = False

    def functions(self):
        """(name, [(type, param)], stateMutability, [output types]) in ABI order."""
        rel = [("address", "payee"), ("uint256", "amount")] + (
            [("bytes32", "memo")] if self.memo else [])
        return [(GETTER, [], "view", ["address"]),
                (PAIR_FN, [("address", "payee")], "payable", []),
                (self.release, rel, "nonpayable", [])]

    def events(self):
        ins = [("address", "payee", True), ("uint256", "amount", False)]
        return [("Deposited", ins), (self.event, ins)]


def drift_of(case):
    return DRIFTS[int(case["id"].split("-")[1]) % len(DRIFTS)]


def spec(kind, new):
    if not new:
        return Spec()
    return {"fn_rename": Spec(release="settle"), "event_rename": Spec(event="Settled"),
            "arity": Spec(memo=True)}[kind]


def server_spec(kind, state):
    return spec(kind, state != "P")


def client_spec(kind, state):
    return spec(kind, state == "C")


def solidity(sp):
    memo = ", bytes32 memo" if sp.memo else ""
    return fill(
        """
        // SPDX-License-Identifier: MIT
        pragma solidity ^0.8.24;

        /// @title Escrow holds a payer's deposit until the arbiter releases it.
        contract Escrow {
            address public arbiter;
            mapping(address => uint256) public balances;

            event Deposited(address indexed payee, uint256 amount);
            event <<event>>(address indexed payee, uint256 amount);

            constructor(address arbiter_) {
                arbiter = arbiter_;
            }

            function deposit(address payee) external payable {
                balances[payee] += msg.value;
                emit Deposited(payee, msg.value);
            }

            function <<release>>(address payee, uint256 amount<<memo>>) external {
                require(msg.sender == arbiter, "arbiter only");
                balances[payee] -= amount;
                payable(payee).transfer(amount);
                emit <<event>>(payee, amount);
            }
        }
        """,
        event=sp.event, release=sp.release, memo=memo,
    )


def human_abi(sp):
    """ethers' human-readable fragments."""
    out = []
    for name, ins, mut, outs in sp.functions():
        args = ", ".join(f"{t} {n}" for t, n in ins)
        tail = "" if mut == "nonpayable" else f" {mut}"
        ret = f" returns ({', '.join(outs)})" if outs else ""
        out.append(f"function {name}({args}){tail}{ret}")
    for name, ins in sp.events():
        args = ", ".join(f"{t}{' indexed' if ix else ''} {n}" for t, n, ix in ins)
        out.append(f"event {name}({args})")
    return out


def json_abi(sp):
    """The ABI as solc writes it."""
    out = []
    for name, ins, mut, outs in sp.functions():
        out.append({"type": "function", "name": name,
                    "inputs": [{"name": n, "type": t, "internalType": t} for t, n in ins],
                    "outputs": [{"name": "", "type": t, "internalType": t} for t in outs],
                    "stateMutability": mut})
    for name, ins in sp.events():
        out.append({"type": "event", "name": name, "anonymous": False,
                    "inputs": [{"name": n, "type": t, "indexed": ix, "internalType": t}
                               for t, n, ix in ins]})
    return out


def abi_lines(sp, indent):
    """The JSON ABI, one entry per line, as the body of an array literal."""
    return ",\n".join(indent + json.dumps(e) for e in json_abi(sp))


def contracts_files():
    """The hardhat project around the contract (contracts/<C>.sol is added per state)."""
    return {
        "package.json": json.dumps({"name": "@sibyl/contracts", "version": "0.1.0",
                                    "private": True,
                                    "devDependencies": {"hardhat": VERSIONS["hardhat"]}},
                                   indent=2) + "\n",
        "hardhat.config.ts": fill(
            """
            import type { HardhatUserConfig } from 'hardhat/config';

            const config: HardhatUserConfig = { solidity: '0.8.24' };

            export default config;
            """),
    }


# ----------------------------------------------------------------------------------- clients

@dataclass
class Out:
    api: dict = field(default_factory=dict)   # api-layer-relative files
    app: dict = field(default_factory=dict)   # app-relative files
    call: tuple = ("api", "")                 # (layer, file) writing the contract calls
    reader: str = ""                          # app-relative file calling the api layer
    deps: dict = field(default_factory=dict)
    dev: dict = field(default_factory=dict)
    exports: list = field(default_factory=list)


def lines(*parts):
    """The parts joined by newlines, a None part (an absent helper) left out, ending in one
    newline; "" is a blank line, and a part's own trailing newline gives one after it."""
    return "\n".join(p for p in parts if p is not None).rstrip("\n") + "\n"


def pascal(name):
    return name[:1].upper() + name[1:]


def ts_addr(ax, o, imports):
    if ax["wiring_source"] == "literal":
        return f"'{ADDRESS}'"
    o.api["addresses.ts"] = f"export const ESCROW_ADDRESS = '{ADDRESS}' as const;\n"
    imports.append("import { ESCROW_ADDRESS } from './addresses';")
    return "ESCROW_ADDRESS"


TS_ETHERS_HEAD = """
const provider = new JsonRpcProvider('http://localhost:8545');
const signer = Wallet.createRandom().connect(provider);
"""


def ts_typechain(sp):
    """A TypeChain (ethers-v6 target) output directory, trimmed to the Escrow contract."""
    memo = ", memo: BytesLike" if sp.memo else ""
    note = "/* Autogenerated file. Do not edit manually. */\n/* eslint-disable */\n"
    return {
        "typechain-types/Escrow.ts": note + fill(
            """
            import type { AddressLike, BaseContract, BigNumberish, BytesLike, ContractTransactionResponse } from 'ethers';

            export interface Escrow extends BaseContract {
              arbiter(): Promise<string>;
              deposit(payee: AddressLike, overrides?: { value?: BigNumberish }): Promise<ContractTransactionResponse>;
              <<release>>(payee: AddressLike, amount: BigNumberish<<memo>>): Promise<ContractTransactionResponse>;
              filters: {
                Deposited(payee?: AddressLike | null): unknown;
                <<event>>(payee?: AddressLike | null): unknown;
              };
            }
            """, release=sp.release, memo=memo, event=sp.event),
        "typechain-types/factories/Escrow__factory.ts": note + fill(
            """
            import { Contract, Interface, type ContractRunner } from 'ethers';
            import type { Escrow } from '../Escrow';

            const _abi = [
            <<abi>>
            ] as const;

            export class Escrow__factory {
              static readonly abi = _abi;

              static createInterface(): Interface {
                return new Interface(_abi);
              }

              static connect(address: string, runner?: ContractRunner | null): Escrow {
                return new Contract(address, _abi, runner) as unknown as Escrow;
              }
            }
            """, abi=abi_lines(sp, "  ")),
        "typechain-types/index.ts": (
            note + "export type { Escrow } from './Escrow';\n"
            "export { Escrow__factory } from './factories/Escrow__factory';\n"),
    }


def ts_api_fns(sp, call_fn, read, watch, wrapper):
    """The api layer's exported functions over one contract handle; `wrapper` routes the two
    writes through a method-name helper, `call_fn(member, args)` writes one direct call."""
    memo_p = ", memo: `0x${string}`" if sp.memo else ""
    memo_a = ", memo" if sp.memo else ""
    if wrapper:
        dep = f"await callEscrow('{PAIR_FN}', [payee], amount);"
        rel = f"await callEscrow('{sp.release}', [payee, amount{memo_a}]);"
    else:
        dep = "await " + call_fn(PAIR_FN, "payee", value=True) + ";"
        rel = "await " + call_fn(sp.release, "payee, amount" + memo_a) + ";"
    return fill(
        """
        export async function deposit(payee: `0x${string}`, amount: bigint): Promise<void> {
          <<dep>>
        }

        export async function <<release>>(payee: `0x${string}`, amount: bigint<<memo_p>>): Promise<void> {
          <<rel>>
        }

        export async function readArbiter(): Promise<string> {
          return <<read>>;
        }

        export function watch<<event>>(handler: (...args: unknown[]) => void): void {
          <<watch>>
        }
        """,
        dep=dep, rel=rel, release=sp.release, memo_p=memo_p, read=read, event=sp.event,
        watch=watch)


def ts(ax, sp):
    contract, call = ax["contract"], ax["wiring_call"]
    o = Out(reader="app.ts")
    imports = []
    addr = ts_addr(ax, o, imports)
    wrapper = call == "wrapper"
    memo_a = ", MEMO" if sp.memo else ""
    if contract == "plain-literal":
        o.deps = {"ethers": VERSIONS["ethers"]}
        abi = ",\n".join(f"  '{f}'" for f in human_abi(sp))
        helper = fill(
            """
            async function callEscrow(method: string, args: unknown[], value?: bigint): Promise<unknown> {
              const fn = escrow.getFunction(method);
              return value === undefined ? fn(...args) : fn(...args, { value });
            }
            """) if wrapper else None
        body = ts_api_fns(
            sp, lambda m, a, value=False: f"escrow.{m}({a}{', { value: amount }' if value else ''})",
            "escrow.arbiter()", f"void escrow.on('{sp.event}', handler);", wrapper)
        o.api["escrow.ts"] = lines(
            "import { Contract, JsonRpcProvider, Wallet } from 'ethers';", *imports, "",
            f"const ESCROW_ABI = [\n{abi},\n];", TS_ETHERS_HEAD.rstrip("\n"),
            f"const escrow = new Contract({addr}, ESCROW_ABI, signer);\n", helper, body)
    elif contract == "schema-object":
        o.deps = {"viem": VERSIONS["viem"]}
        o.api["abi.ts"] = f"export const escrowAbi = [\n{abi_lines(sp, '  ')},\n] as const;\n"
        opts = f"address: {addr}, abi: escrowAbi"
        helper = fill(
            """
            async function callEscrow(functionName: string, args: readonly unknown[], value?: bigint): Promise<void> {
              await walletClient.writeContract({ account: PAYER, <<opts>>, functionName, args, value } as never);
            }
            """, opts=opts) if wrapper else None

        def write(m, a, value=False):
            v = ", value: amount" if value else ""
            return (f"walletClient.writeContract({{ account: PAYER, {opts}, functionName: '{m}', "
                    f"args: [{a}]{v} }})")

        body = ts_api_fns(
            sp, write, f"publicClient.readContract({{ {opts}, functionName: '{GETTER}' }})",
            f"publicClient.watchContractEvent({{ {opts}, eventName: '{sp.event}', "
            "onLogs: handler });", wrapper)
        o.api["escrow.ts"] = lines(
            "import { createPublicClient, createWalletClient, http } from 'viem';",
            "import { hardhat } from 'viem/chains';", "import { escrowAbi } from './abi';",
            *imports, "",
            f"const PAYER = '{PAYER}';",
            "const publicClient = createPublicClient({ chain: hardhat, transport: http() });",
            "const walletClient = createWalletClient({ chain: hardhat, transport: http() });\n",
            helper, body)
    else:  # generated: a TypeChain binding
        o.deps = {"ethers": VERSIONS["ethers"]}
        o.dev = {"typechain": VERSIONS["typechain"],
                 "@typechain/ethers-v6": VERSIONS["@typechain/ethers-v6"]}
        o.api.update(ts_typechain(sp))
        head = ["import { JsonRpcProvider, Wallet } from 'ethers';",
                "import { Escrow__factory } from './typechain-types';", *imports, "",
                TS_ETHERS_HEAD.strip("\n"),
                f"const escrow = Escrow__factory.connect({addr}, signer);\n"]
        if call == "generated-sdk":
            o.api["contracts.ts"] = lines(*head[:-1], "export " + head[-1])
            o.exports = ["contracts", "typechain-types"]
            o.call = ("app", "app.ts")
            memo = "\nconst MEMO = `0x${'00'.repeat(32)}` as const;\n" if sp.memo else ""
            o.app["app.ts"] = fill(
                """
                <<imp>>
                <<memo>>
                export async function checkout(payee: `0x${string}`, amount: bigint): Promise<string> {
                  await escrow.deposit(payee, { value: amount });
                  await escrow.<<release>>(payee, amount<<memo_a>>);
                  void escrow.on(escrow.filters.<<event>>(), () => undefined);
                  return escrow.arbiter();
                }
                """, imp=app_import(ax, ["escrow"], "contracts"), memo=memo,
                release=sp.release, memo_a=memo_a, event=sp.event)
            return o
        helper = fill(
            """
            async function callEscrow(method: string, args: unknown[], value?: bigint): Promise<unknown> {
              const fn = escrow.getFunction(method);
              return value === undefined ? fn(...args) : fn(...args, { value });
            }
            """) if wrapper else None
        body = ts_api_fns(
            sp, lambda m, a, value=False: f"escrow.{m}({a}{', { value: amount }' if value else ''})",
            "escrow.arbiter()", f"void escrow.on(escrow.filters.{sp.event}(), handler);", wrapper)
        o.api["escrow.ts"] = lines(*head, helper, body)
    o.call = ("api", "escrow.ts")
    o.exports = ["escrow"]
    memo = "\nconst MEMO = `0x${'00'.repeat(32)}` as const;\n" if sp.memo else ""
    o.app["app.ts"] = fill(
        """
        <<imp>>
        <<memo>>
        export async function checkout(payee: `0x${string}`, amount: bigint): Promise<string> {
          await deposit(payee, amount);
          await <<release>>(payee, amount<<memo_a>>);
          watch<<event>>(() => undefined);
          return readArbiter();
        }
        """, imp=app_import(ax, ["deposit", sp.release, "readArbiter",
                                       f"watch{sp.event}"], "escrow"),
        memo=memo, release=sp.release, memo_a=memo_a, event=sp.event)
    return o


def app_import(ax, names, module):
    if ax["packaging"] == "workspace":
        return f"import {{ {', '.join(names)} }} from '@sibyl/api-client';"
    return f"import {{ {', '.join(names)} }} from './api/{module}';"


# ---------------------------------------------------------------------------------------- Go

def go_params(sp):
    """(Go parameter list after opts, its argument names) of the drifting function."""
    ps = "payee common.Address, amount *big.Int" + (", memo [32]byte" if sp.memo else "")
    return ps, "payee, amount" + (", memo" if sp.memo else "")


def go_binding(sp, pkg):
    """An abigen v1 binding of the contract, trimmed to its members."""
    rel, (ps, args) = pascal(sp.release), go_params(sp)
    meta = json.dumps(json.dumps(json_abi(sp), separators=(",", ":")))
    ev = sp.event
    return fill(
        """
        // Code generated - DO NOT EDIT.
        // This file is a generated binding and any manual changes will be lost.

        package <<pkg>>

        import (
        \t"math/big"

        \t"github.com/ethereum/go-ethereum/accounts/abi"
        \t"github.com/ethereum/go-ethereum/accounts/abi/bind"
        \t"github.com/ethereum/go-ethereum/common"
        \t"github.com/ethereum/go-ethereum/core/types"
        \t"github.com/ethereum/go-ethereum/event"
        )

        // EscrowMetaData contains all meta data concerning the Escrow contract.
        var EscrowMetaData = &bind.MetaData{
        \tABI: <<meta>>,
        }

        // Escrow is an auto generated Go binding around an Ethereum contract.
        type Escrow struct {
        \tEscrowCaller     // Read-only binding to the contract
        \tEscrowTransactor // Write-only binding to the contract
        \tEscrowFilterer   // Log filterer for contract events
        }

        // EscrowCaller is an auto generated read-only Go binding around an Ethereum contract.
        type EscrowCaller struct {
        \tcontract *bind.BoundContract
        }

        // EscrowTransactor is an auto generated write-only Go binding around an Ethereum contract.
        type EscrowTransactor struct {
        \tcontract *bind.BoundContract
        }

        // EscrowFilterer is an auto generated log filtering Go binding around an Ethereum contract events.
        type EscrowFilterer struct {
        \tcontract *bind.BoundContract
        }

        // EscrowTransactorRaw is an auto generated low-level write-only Go binding around an Ethereum contract.
        type EscrowTransactorRaw struct {
        \tContract *EscrowTransactor
        }

        // NewEscrow creates a new instance of Escrow, bound to a specific deployed contract.
        func NewEscrow(address common.Address, backend bind.ContractBackend) (*Escrow, error) {
        \tparsed, err := EscrowMetaData.GetAbi()
        \tif err != nil {
        \t\treturn nil, err
        \t}
        \tcontract := bind.NewBoundContract(address, *parsed, backend, backend, backend)
        \treturn &Escrow{EscrowCaller: EscrowCaller{contract: contract}, EscrowTransactor: EscrowTransactor{contract: contract}, EscrowFilterer: EscrowFilterer{contract: contract}}, nil
        }

        // Arbiter is a free data retrieval call binding the contract method arbiter.
        func (_Escrow *EscrowCaller) Arbiter(opts *bind.CallOpts) (common.Address, error) {
        \tvar out []interface{}
        \terr := _Escrow.contract.Call(opts, &out, "arbiter")
        \tif err != nil {
        \t\treturn *new(common.Address), err
        \t}
        \treturn *abi.ConvertType(out[0], new(common.Address)).(*common.Address), nil
        }

        // Deposit is a paid mutator transaction binding the contract method deposit.
        func (_Escrow *EscrowTransactor) Deposit(opts *bind.TransactOpts, payee common.Address) (*types.Transaction, error) {
        \treturn _Escrow.contract.Transact(opts, "deposit", payee)
        }

        // <<rel>> is a paid mutator transaction binding the contract method <<release>>.
        func (_Escrow *EscrowTransactor) <<rel>>(opts *bind.TransactOpts, <<ps>>) (*types.Transaction, error) {
        \treturn _Escrow.contract.Transact(opts, "<<release>>", <<args>>)
        }

        // Transact invokes the (paid) contract method with params as input values.
        func (_Escrow *EscrowTransactorRaw) Transact(opts *bind.TransactOpts, method string, params ...interface{}) (*types.Transaction, error) {
        \treturn _Escrow.Contract.contract.Transact(opts, method, params...)
        }

        // Escrow<<ev>> represents a <<ev>> event raised by the Escrow contract.
        type Escrow<<ev>> struct {
        \tPayee  common.Address
        \tAmount *big.Int
        \tRaw    types.Log // Blockchain specific contextual infos
        }

        // Watch<<ev>> is a free log subscription operation binding the contract event <<ev>>.
        func (_Escrow *EscrowFilterer) Watch<<ev>>(opts *bind.WatchOpts, sink chan<- *Escrow<<ev>>) (event.Subscription, error) {
        \tlogs, sub, err := _Escrow.contract.WatchLogs(opts, "<<ev>>")
        \tif err != nil {
        \t\treturn nil, err
        \t}
        \treturn event.NewSubscription(func(quit <-chan struct{}) error {
        \t\tdefer sub.Unsubscribe()
        \t\tfor {
        \t\t\tselect {
        \t\t\tcase log := <-logs:
        \t\t\t\tev := new(Escrow<<ev>>)
        \t\t\t\tif err := _Escrow.contract.UnpackLog(ev, "<<ev>>", log); err != nil {
        \t\t\t\t\treturn err
        \t\t\t\t}
        \t\t\t\tev.Raw = log
        \t\t\t\tselect {
        \t\t\t\tcase sink <- ev:
        \t\t\t\tcase err := <-sub.Err():
        \t\t\t\t\treturn err
        \t\t\t\tcase <-quit:
        \t\t\t\t\treturn nil
        \t\t\t\t}
        \t\t\tcase err := <-sub.Err():
        \t\t\t\treturn err
        \t\t\tcase <-quit:
        \t\t\t\treturn nil
        \t\t\t}
        \t\t}
        \t}), nil
        }
        """,
        pkg=pkg, meta=meta, rel=rel, release=sp.release, ps=ps, args=args, ev=ev)


def go_imports(paths):
    std = sorted(p for p in paths if "." not in p.split("/")[0])
    ext = sorted(p for p in paths if "." in p.split("/")[0])
    body = "".join(f'\t"{p}"\n' for p in std)
    if std and ext:
        body += "\n"
    body += "".join(f'\t"{p}"\n' for p in ext)
    return f"import (\n{body})\n"


def go_client(sp, contract, wrapper):
    """chain/escrow.go: EscrowClient, the api layer the app calls (Deposit, the drifting
    function, Arbiter, Watch<Event>)."""
    rel, (ps, args) = pascal(sp.release), go_params(sp)
    ev = sp.event
    imports = ["math/big", f"{GETH}/accounts/abi/bind", f"{GETH}/common",
               f"{GETH}/core/types", f"{GETH}/event"]
    if contract == "generated":
        fields = "\tescrow *Escrow\n" + ("\traw    *EscrowTransactorRaw\n" if wrapper else "")
        build = ("\te, err := NewEscrow(addr, backend)\n\tif err != nil {\n\t\treturn nil, err\n"
                 "\t}\n\treturn &EscrowClient{escrow: e"
                 + (", raw: &EscrowTransactorRaw{Contract: &e.EscrowTransactor}" if wrapper
                    else "") + "}, nil\n")
        submit = "c.raw.Transact(opts, method, args...)"
        dep, relc = "c.escrow.Deposit(opts, payee)", f"c.escrow.{rel}(opts, {args})"
        read = "\treturn c.escrow.Arbiter(opts)\n"
        watch = f"\treturn c.escrow.Watch{ev}(opts, make(chan *Escrow{ev}))\n"
        head = ""
    else:
        imports += ["strings", f"{GETH}/accounts/abi"]
        fields = "\tcontract *bind.BoundContract\n"
        build = ("\tparsed, err := abi.JSON(strings.NewReader(EscrowABI))\n\tif err != nil {\n"
                 "\t\treturn nil, err\n\t}\n\treturn &EscrowClient{contract: "
                 "bind.NewBoundContract(addr, parsed, backend, backend, backend)}, nil\n")
        submit = "c.contract.Transact(opts, method, args...)"
        dep = f'c.contract.Transact(opts, "{PAIR_FN}", payee)'
        relc = f'c.contract.Transact(opts, "{sp.release}", {args})'
        read = ("\tvar out []interface{}\n"
                f'\tif err := c.contract.Call(opts, &out, "{GETTER}"); err != nil {{\n'
                "\t\treturn common.Address{}, err\n\t}\n"
                "\treturn *abi.ConvertType(out[0], new(common.Address)).(*common.Address), nil\n")
        watch = (f'\t_, sub, err := c.contract.WatchLogs(opts, "{ev}")\n\treturn sub, err\n')
        head = (f"// EscrowABI is the Escrow contract's ABI.\nconst EscrowABI = `[\n"
                f"{abi_lines(sp, '  ')}\n]`\n\n") if contract == "plain-literal" else ""
    if wrapper:
        dep = f'c.submit(opts, "{PAIR_FN}", payee)'
        relc = f'c.submit(opts, "{sp.release}", {args})'
    helper = ("// submit sends one paid call of the contract method named method.\n"
              "func (c *EscrowClient) submit(opts *bind.TransactOpts, method string, "
              "args ...interface{}) (*types.Transaction, error) {\n"
              f"\treturn {submit}\n}}\n\n") if wrapper else ""
    return (
        f"package chain\n\n{go_imports(imports)}\n{head}"
        "// EscrowClient pays into and releases from the Escrow contract.\n"
        f"type EscrowClient struct {{\n{fields}}}\n\n"
        "// NewEscrowClient binds the Escrow contract deployed at addr.\n"
        "func NewEscrowClient(addr common.Address, backend bind.ContractBackend) "
        f"(*EscrowClient, error) {{\n{build}}}\n\n{helper}"
        "// Deposit pays into the escrow for payee.\n"
        "func (c *EscrowClient) Deposit(opts *bind.TransactOpts, payee common.Address) "
        f"(*types.Transaction, error) {{\n\treturn {dep}\n}}\n\n"
        f"// {rel} pays amount out of the escrow to payee.\n"
        f"func (c *EscrowClient) {rel}(opts *bind.TransactOpts, {ps}) "
        f"(*types.Transaction, error) {{\n\treturn {relc}\n}}\n\n"
        "// Arbiter reads the escrow's arbiter.\n"
        "func (c *EscrowClient) Arbiter(opts *bind.CallOpts) (common.Address, error) {\n"
        f"{read}}}\n\n"
        f"// Watch{ev} subscribes to the escrow's {ev} events.\n"
        f"func (c *EscrowClient) Watch{ev}(opts *bind.WatchOpts) (event.Subscription, error) "
        f"{{\n{watch}}}\n"
    )


def go_main(sp, ax, module):
    """main.go: the app, calling the api layer (chain.EscrowClient, or the abigen package)."""
    sdk = ax["wiring_call"] == "generated-sdk"
    pkg = "bindings" if sdk else "chain"
    addr = (f'"{ADDRESS}"' if ax["wiring_source"] == "literal" else "escrowAddress")
    rel = pascal(sp.release)
    memo = ", memo" if sp.memo else ""
    memo_decl = "\tvar memo [32]byte // the release reference\n" if sp.memo else ""
    ev = sp.event
    ctor = "bindings.NewEscrow" if sdk else "chain.NewEscrowClient"
    watch = (f"escrow.Watch{ev}(nil, make(chan *bindings.Escrow{ev}))" if sdk
             else f"escrow.Watch{ev}(nil)")
    imports = ["log", "math/big", f"{GETH}/accounts/abi/bind", f"{GETH}/common",
               f"{GETH}/ethclient", f"{module}/{pkg}"]
    return (
        f"package main\n\n{go_imports(imports)}\n"
        "func main() {\n"
        '\tbackend, err := ethclient.Dial("http://localhost:8545")\n'
        "\tif err != nil {\n\t\tlog.Fatal(err)\n\t}\n"
        f"\tescrow, err := {ctor}(common.HexToAddress({addr}), backend)\n"
        "\tif err != nil {\n\t\tlog.Fatal(err)\n\t}\n"
        f'\tpayee := common.HexToAddress("{PAYEE}")\n'
        "\tif _, err := escrow.Deposit(&bind.TransactOpts{}, payee); err != nil {\n"
        "\t\tlog.Fatal(err)\n\t}\n"
        f"{memo_decl}\tif _, err := escrow.{rel}(&bind.TransactOpts{{}}, payee, big.NewInt(1){memo}); "
        "err != nil {\n\t\tlog.Fatal(err)\n\t}\n"
        f"\tsub, err := {watch}\n"
        "\tif err != nil {\n\t\tlog.Fatal(err)\n\t}\n"
        "\tdefer sub.Unsubscribe()\n"
        "\tarbiter, err := escrow.Arbiter(nil)\n"
        "\tif err != nil {\n\t\tlog.Fatal(err)\n\t}\n"
        '\tlog.Printf("escrow arbiter %s", arbiter.Hex())\n'
        "}\n"
    )


def go(ax, sp, module):
    contract, call = ax["contract"], ax["wiring_call"]
    o = Out(reader="main.go")
    if call == "generated-sdk":
        o.api["bindings/escrow.go"] = go_binding(sp, "bindings")
        o.call = ("api", "bindings/escrow.go")
    else:
        o.api["chain/escrow.go"] = go_client(sp, contract, call == "wrapper")
        if contract == "schema-object":
            o.api["chain/abi.go"] = (f"package chain\n\n// EscrowABI is the Escrow contract's "
                                     f"ABI.\nconst EscrowABI = `[\n{abi_lines(sp, '  ')}\n]`\n")
        if contract == "generated":
            o.api["chain/escrow_binding.go"] = go_binding(sp, "chain")
            o.call = ("api", "chain/escrow_binding.go")
        else:
            o.call = ("api", "chain/escrow.go")
    o.app["main.go"] = go_main(sp, ax, module)
    if ax["wiring_source"] == "const":
        o.app["address.go"] = f'package main\n\nconst escrowAddress = "{ADDRESS}"\n'
    return o


# --------------------------------------------------------------------------------- placement

def _j(*parts):
    return "/".join(p.strip("/") for p in parts if p)


def layout(packaging, lang):
    """(client repo, api base, app base, server repo, contracts base)."""
    if packaging == "workspace":
        return "repo", "packages/api-client", "apps/web", "repo", (
            "packages/contracts" if lang == "ts" else "contracts")
    if packaging == "multi-repo":
        return "client", "", "", "server", ""
    return "repo", "client", "client", "repo", "contracts"


def ts_manifest(name, deps, dev=None, extra=None):
    return json.dumps({"name": name, "version": "0.1.0", "private": True, **(extra or {}),
                       "dependencies": deps, **({"devDependencies": dev} if dev else {})},
                      indent=2) + "\n"


def go_mod(module, requires):
    req = "".join(f"\t{m} {v}\n" for m, v in requires)
    return f"module {module}\n\ngo 1.22\n" + (f"\nrequire (\n{req})\n" if req else "")


def _render(ax, kind, state):
    lang = ax["client"]
    crepo, api, app, srepo, sb = layout(ax["packaging"], lang)
    ws = ax["packaging"] == "workspace"
    repos = {crepo: {}, srepo: {}}
    c = repos[crepo]
    sp = client_spec(kind, state)
    if lang == "ts":
        o = ts(ax, sp)
        if ws:
            api_src, app_src = _j(api, "src"), _j(app, "src")
            c["package.json"] = ts_manifest("sibyl", {})
            c["pnpm-workspace.yaml"] = "packages:\n  - 'apps/*'\n  - 'packages/*'\n"
            c[_j(api, "package.json")] = ts_manifest(
                "@sibyl/api-client", o.deps, o.dev,
                {"main": "src/index.ts", "types": "src/index.ts"})
            c[_j(api, "tsconfig.json")] = rest_servers.TSCONFIG
            c[_j(api_src, "index.ts")] = "".join(f"export * from './{m}';\n" for m in o.exports)
            c[_j(app, "package.json")] = ts_manifest(
                "@sibyl/web", {"@sibyl/api-client": "workspace:*"})
            c[_j(app, "tsconfig.json")] = rest_servers.TSCONFIG
        else:
            base = "client" if ax["packaging"] == "single" else ""
            api_src, app_src = _j(base, "src/api"), _j(base, "src")
            c[_j(base, "package.json")] = ts_manifest("sibyl-web", o.deps, o.dev)
            c[_j(base, "tsconfig.json")] = rest_servers.TSCONFIG
    elif lang == "go":
        req = [(GETH, VERSIONS["go-ethereum"])]
        if ws:
            module = "example.com/sibyl/apiclient"
            c["go.work"] = "go 1.22\n\nuse (\n\t./apps/web\n\t./packages/api-client\n)\n"
            c[_j(api, "go.mod")] = go_mod(module, req)
            c[_j(app, "go.mod")] = go_mod("example.com/sibyl/web", req)
        else:
            module = "example.com/sibyl/client"
            c[_j(api, "go.mod")] = go_mod(module, req)
        o = go(ax, sp, module)
        api_src, app_src = api, app
    else:
        raise KeyError(f"no evm client template for {lang!r}")
    for rel, text in o.api.items():
        c[_j(api_src, rel)] = text
    for rel, text in o.app.items():
        c[_j(app_src, rel)] = text
    s = repos[srepo]
    for rel, text in contracts_files().items():
        s[_j(sb, rel)] = text
    sol = _j(sb, "contracts/Escrow.sol")
    s[sol] = solidity(server_spec(kind, state))
    layer, rel = o.call
    meta = {
        "client": {"repo": crepo, "file": _j(api_src if layer == "api" else app_src, rel)},
        "reader": {"repo": crepo, "file": _j(app_src, o.reader)},
        "server": {"repo": srepo, "route_file": sol, "handler": PAIR_FN, "handler_file": sol,
                   "model_file": sol, "pair_files": [sol]},
    }
    return {r: dict(sorted(fs.items())) for r, fs in sorted(repos.items())}, meta


def render(case, state):
    return _render(case["axes"], drift_of(case), state)[0]


# ------------------------------------------------------------------------------ the answer

def _changed(a, b):
    out = []
    for repo in sorted(set(a) | set(b)):
        fa, fb = a.get(repo, {}), b.get(repo, {})
        out += [{"repo": repo, "file": p} for p in sorted(set(fa) | set(fb))
                if fa.get(p) != fb.get(p)]
    return out


def stale_name(kind):
    """(member kind, the S-stale name, a regex finding it in a client line)."""
    if kind == "event_rename":
        # abigen and the api layers prefix the event: WatchReleased, EscrowReleased.
        return "event", "Released", re.compile(r"Released\b")
    return "function", "release", re.compile(r"\b[Rr]elease\b")


NOTES = {
    "fn_rename": ("the contract renames release to settle; the client still calls release",
                  "the client follows: it calls settle"),
    "event_rename": ("the contract renames the event Released to Settled; the client still "
                     "subscribes to Released", "the client follows: it subscribes to Settled"),
    "arity": ("release takes a third argument, bytes32 memo; the client still passes two",
              "the client follows: it passes the memo"),
}


def expected(case):
    ax, kind = case["axes"], drift_of(case)
    renders = {s: _render(ax, kind, s)[0] for s in STATES}
    meta = _render(ax, kind, "P")[1]
    multi = ax["packaging"] == "multi-repo"
    client_files = _changed(renders["S"], renders["C"])
    member_kind, old, rx = stale_name(kind)
    sites = []
    for ref in client_files:
        text = renders["S"][ref["repo"]].get(ref["file"], "")
        sites += [{"repo": ref["repo"], "file": ref["file"], "line": i, "text": line.strip()}
                  for i, line in enumerate(text.splitlines(), 1) if rx.search(line)]
    new = spec(kind, True)
    cl, srv = meta["client"], meta["server"]
    sol = srv["route_file"]
    drift = {"kind": kind, "member_kind": member_kind, "old": old,
             "new": {"fn_rename": new.release, "event_rename": new.event,
                     "arity": new.release}[kind],
             "server_files": _changed(renders["P"], renders["S"]),
             "client_files": client_files, "client_sites": sites,
             "flag_at": "S", "clear_at": "C"}
    if kind == "arity":
        drift["arity"] = {"old": 2, "new": 3}
    return {
        "id": case["id"],
        "transport": "evm",
        "axes": ax,
        "repos": sorted(renders["P"]),
        "primary": "server" if multi else "repo",
        "with": ["client"] if multi else [],
        "exchange": {"key": CONTRACT, "contract": CONTRACT, "function": PAIR_FN,
                     "procedure": f"evm:{CONTRACT}.{PAIR_FN}",
                     "match": [rf"^evm:{CONTRACT}\.{PAIR_FN}$"]},
        "pair": {"category": CATEGORY, "client": cl, "server": srv},
        "reader": meta["reader"],
        # What the exchange holds at P, before any drift.
        "edges": [
            *({"category": "RPC_CALLS", "client": cl, "server_file": sol,
               "procedure": f"evm:{CONTRACT}.{m}"} for m in (PAIR_FN, "release", GETTER)),
            {"category": "EVENT_FLOWS", "client": cl, "server_file": sol,
             "event": f"{CONTRACT}.Released"},
        ],
        "drift": drift,
        "states": {"P": "base: the client and the Escrow contract agree",
                   "S": NOTES[kind][0], "C": NOTES[kind][1]},
    }
