"""What the gRPC, GraphQL, queue and WebSocket transports share: placing a case's client and
server by `packaging` (through rest.py's layout and client placement, so every transport lays a
workspace out the same way), the server manifests, and the expected answer read off the renders.

A transport module hands `assemble` its client function (`client(ax, field, pack)` ->
rest_clients.Out, the api layer and the app file) and its server function
(`server(ax, field)` -> (files, meta), meta naming `route_file` / `handler` / `handler_file` /
`model_file` / `pair_files` / `eco`, and `settings` for a Gradle server). `expected` derives the
drift's file sets from the P / S / C renders, never from a hand-written list."""

import json

from . import rest, rest_servers
from .common import NEW, OLD, client_field, fill, join, server_field

STATES = ("P", "S", "C")


def notes(what):
    return {
        "P": f"base: client and server agree on {OLD.camel} ({what})",
        "S": f"the server renames {OLD.camel} to {NEW.camel}; the client is stale",
        "C": f"the client follows: it reads {NEW.camel}",
    }


# --------------------------------------------------------------------------- client placement

SWIFT_DEPS = {
    "grpc-swift": ('.package(url: "https://github.com/grpc/grpc-swift.git", from: "1.23.0")',
                   '.product(name: "GRPC", package: "grpc-swift")'),
    "swift-protobuf": ('.package(url: "https://github.com/apple/swift-protobuf.git", '
                       'from: "1.28.0")',
                       '.product(name: "SwiftProtobuf", package: "swift-protobuf")'),
    "apollo-ios": ('.package(url: "https://github.com/apollographql/apollo-ios.git", '
                   'from: "1.15.0")',
                   '.product(name: "Apollo", package: "apollo-ios")'),
}


def swift_package(name, deps):
    """A SwiftPM manifest whose one target uses the products of `deps` (SWIFT_DEPS keys);
    rest.py's own manifest takes none, so a transport that needs a package rewrites it."""
    pk = "".join(f"        {SWIFT_DEPS[d][0]},\n" for d in deps)
    pr = ", ".join(SWIFT_DEPS[d][1] for d in deps)
    return (
        "// swift-tools-version:5.9\nimport PackageDescription\n\n"
        "let package = Package(\n"
        f'    name: "{name}",\n'
        "    platforms: [.macOS(.v12), .iOS(.v15)],\n"
        f'    products: [.library(name: "{name}", targets: ["{name}"])],\n'
        f"    dependencies: [\n{pk}    ],\n"
        f'    targets: [.target(name: "{name}", dependencies: [{pr}])]\n'
        ")\n"
    )


def assemble(ax, state, client, server):
    crepo, cb, api, _, srepo, sb = rest.layout(ax["packaging"])
    pack = "workspace" if ax["packaging"] == "workspace" else "single"
    sfiles, smeta = server(ax, server_field(state))
    shared = pack == "workspace" and smeta["eco"] == rest.ECO_OF_CLIENT[ax["client"]]
    repos = {crepo: {}, srepo: {}}
    out = client(ax, client_field(state), pack)
    swift = [d for d in out.deps if d in SWIFT_DEPS]
    out.deps = [d for d in out.deps if d not in SWIFT_DEPS]
    call, reader = rest.place_client(ax, out, repos[crepo], pack,
                                     smeta["eco"] if shared else None)
    if swift:
        name, base = ("ApiClient", api) if pack == "workspace" else ("Client", cb)
        repos[crepo][rest._j(base, "Package.swift")] = swift_package(name, swift)
    for rel, text in sfiles.items():
        repos[srepo][rest._j(sb, rel)] = text
    if "settings" in smeta and not shared:
        repos[srepo][rest._j(sb, "settings.gradle.kts")] = smeta["settings"]
    meta = {
        "client": {"repo": crepo, "file": call},
        "reader": {"repo": crepo, "file": reader},
        "server": {"repo": srepo, "route_file": rest._j(sb, smeta["route_file"]),
                   "handler": smeta["handler"], "handler_file": rest._j(sb, smeta["handler_file"]),
                   "model_file": rest._j(sb, smeta["model_file"]),
                   "pair_files": [rest._j(sb, p) for p in smeta["pair_files"]]},
    }
    return {r: dict(sorted(fs.items())) for r, fs in sorted(repos.items())}, meta


def expected(case, transport, category, exchange, client, server, what):
    """The answer glia must give. `exchange` is the transport's channel: its `key`, its `match`
    list (any of which one end of the pair must name) and whatever else describes it."""
    ax = case["axes"]
    renders = {s: assemble(ax, s, client, server)[0] for s in STATES}
    meta = assemble(ax, "S", client, server)[1]
    multi = ax["packaging"] == "multi-repo"
    return {
        "id": case["id"],
        "transport": transport,
        "axes": ax,
        "repos": sorted(renders["P"]),
        "primary": "server" if multi else "repo",
        "with": ["client"] if multi else [],
        "exchange": exchange,
        "pair": {"category": category, "client": meta["client"], "server": meta["server"]},
        "reader": meta["reader"],
        "drift": {
            "field": {"old": OLD.camel, "new": NEW.camel},
            "direction": "response",
            "server_files": rest._changed(renders["P"], renders["S"]),
            "client_files": rest._changed(renders["S"], renders["C"]),
            "flag_at": "S",
            "clear_at": "C",
        },
        "states": notes(what),
    }


# ----------------------------------------------------------------------------- server manifests

def go_mod(requires):
    req = "".join(f"\t{m} {v}\n" for m, v in requires)
    return f"module example.com/sibyl/api\n\ngo 1.22\n\nrequire (\n{req})\n"


def node(deps, dev=None):
    """package.json + tsconfig.json of a TypeScript server."""
    return {"package.json": json.dumps({"name": "sibyl-api", "version": "0.1.0",
                                        "private": True, "dependencies": deps,
                                        "devDependencies": {"typescript": "^5.6.0",
                                                            **(dev or {})}},
                                       indent=2) + "\n",
            "tsconfig.json": rest_servers.TSCONFIG}


def gradle_spring(deps, extra_plugins=(), extra=""):
    plugins = ["    java", '    id("org.springframework.boot") version "3.3.4"',
               '    id("io.spring.dependency-management") version "1.1.6"', *extra_plugins]
    lines = ["plugins {", *plugins, "}", "", "repositories {", "    mavenCentral()", "}", "",
             "dependencies {", *[f'    implementation("{d}")' for d in deps], "}"]
    return "\n".join(lines) + "\n" + extra


GRADLE_SETTINGS = 'rootProject.name = "sibyl-api"\n'


def pyproject(deps):
    dl = ", ".join(json.dumps(d) for d in deps)
    return (f'[project]\nname = "sibyl-api"\nversion = "0.1.0"\nrequires-python = ">=3.11"\n'
            f"dependencies = [{dl}]\n")


def cargo(deps, build=None):
    lines = ["[package]", 'name = "sibyl-api"', 'version = "0.1.0"', 'edition = "2021"', "",
             "[dependencies]", *[f"{k} = {v}" for k, v in deps]]
    if build:
        lines += ["", "[build-dependencies]", *[f"{k} = {v}" for k, v in build]]
    return "\n".join(lines) + "\n"


def csproj(packages, items=""):
    refs = "".join(f'    <PackageReference Include="{n}" Version="{v}" />\n' for n, v in packages)
    return fill(
        """
        <Project Sdk="Microsoft.NET.Sdk.Web">
          <PropertyGroup>
            <TargetFramework>net8.0</TargetFramework>
            <RootNamespace>Sibyl.Api</RootNamespace>
            <Nullable>enable</Nullable>
            <ImplicitUsings>enable</ImplicitUsings>
          </PropertyGroup>
          <ItemGroup>
        <<refs>><<items>>  </ItemGroup>
        </Project>
        """,
        refs=refs, items=items,
    )


def pick(ws, literal, const, enum=None):
    """The wiring_source form; one a stack cannot express (None) fails loudly, never a default."""
    v = {"literal": literal, "const": const, "enum-prefix": enum}[ws]
    if v is None:
        raise KeyError(f"wiring_source {ws!r} has no form here")
    return v


# ------------------------------------------------------------------- client-side shared files

def konst(lang, name, value):
    """A constant holding a channel piece: (api-relative file, its text, the import line the
    using api file needs, or None when the language needs none)."""
    if lang == "ts":
        return "routes.ts", f"export const {name} = '{value}';\n", \
            f"import {{ {name} }} from './routes';"
    if lang == "dart":
        return "routes.dart", f"const {name} = '{value}';\n", "import 'routes.dart';"
    if lang == "kotlin":
        return (KT_API + "Routes.kt", f'package com.sibyl.api\n\nconst val {name} = "{value}"\n',
                None)
    if lang == "swift":
        return "Routes.swift", f'public let {name} = "{value}"\n', None
    if lang == "python":
        return "routes.py", f'{name} = "{value}"\n', f"from .routes import {name}"
    raise KeyError(lang)


def route_key(lang, enum="RouteKey"):
    """immich's RouteKey enum (or another `enum` name), one member `users`: (file, text, import
    line or None, the expression that reads the member's string)."""
    if lang == "ts":
        return ("routes.ts", f"export enum {enum} {{\n  Users = 'users',\n}}\n",
                f"import {{ {enum} }} from './routes';", f"{enum}.Users")
    if lang == "dart":
        return ("routes.dart", f"enum {enum} {{\n  users('users');\n\n"
                f"  const {enum}(this.segment);\n\n  final String segment;\n}}\n",
                "import 'routes.dart';", f"{enum}.users.segment")
    if lang == "kotlin":
        return (KT_API + "Routes.kt", f"package com.sibyl.api\n\nenum class {enum}(val segment: "
                'String) {\n    USERS("users"),\n}\n', None, f"{enum}.USERS.segment")
    if lang == "swift":
        return ("Routes.swift", f"public enum {enum}: String {{\n    case users\n}}\n", None,
                f"{enum}.users.rawValue")
    if lang == "python":
        return ("routes.py", f'from enum import StrEnum\n\n\nclass {enum}(StrEnum):\n'
                '    USERS = "users"\n', f"from .routes import {enum}", f"{enum}.USERS")
    raise KeyError(lang)


def block(*parts):
    """`join` over parts whose trailing newlines are dropped first (fill() output ends in one)."""
    return join(*(p.rstrip("\n") if isinstance(p, str) else p for p in parts))


def app_import(lang, pack, module, names):
    """The app file's import of `names` from the api layer's `module` (None: no line needed)."""
    if lang == "ts":
        spec = "@sibyl/api-client" if pack == "workspace" else "./api/" + module
        return f"import {{ {', '.join(names)} }} from '{spec}';"
    if lang == "dart":
        spec = "package:api_client/api_client.dart" if pack == "workspace" else "api/" + module
        return f"import '{spec}';"
    if lang == "python":
        spec = ("sibyl_api_client." if pack == "workspace" else ".api.") + module
        return f"from {spec} import {', '.join(names)}"
    if lang == "kotlin":
        return "\n".join(f"import com.sibyl.api.{n}" for n in names)
    if lang == "swift":
        return "import ApiClient" if pack == "workspace" else None
    raise KeyError(lang)


KT_API = "com/sibyl/api/"


def strlit(lang, parts):
    """A string expression from ("lit", text) / ("expr", code) parts, in the language's own
    syntax: a plain literal when every part is literal, else its interpolation form."""
    if len(parts) == 1 and parts[0][0] == "expr":
        return parts[0][1]
    if all(k == "lit" for k, _ in parts):
        text = "".join(v for _, v in parts)
        return f"'{text}'" if lang in ("ts", "dart") else f'"{text}"'
    wrap = {"ts": ("`", "${", "}", "`"), "dart": ("'", "${", "}", "'"),
            "kotlin": ('"', "${", "}", '"'), "swift": ('"', "\\(", ")", '"'),
            "python": ('f"', "{", "}", '"')}[lang]
    body = "".join(v if k == "lit" else wrap[1] + v + wrap[2] for k, v in parts)
    return wrap[0] + body + wrap[3]


CONST_NAME = {"ts": "{U}", "dart": "{c}", "kotlin": "{U}", "swift": "{c}", "python": "{U}"}


def const_name(lang, upper):
    """SCREAMING_CASE where the language writes constants so, lowerCamel where it does not."""
    if CONST_NAME[lang] == "{U}":
        return upper
    head, *rest = upper.lower().split("_")
    return head + "".join(w.title() for w in rest)


def channel_parts(lang, ws, o, imports, literal, const, enum):
    """The parts of a channel string for `wiring_source`, writing the constant or enum file it
    reads into the api layer. `const` is (CONST_NAME, its value, the literal tail after it);
    `enum` is (enum name, the literal head before the member, the literal tail after it)."""
    if ws == "literal":
        return [("lit", literal)]
    if ws == "const":
        upper, value, tail = const
        name = const_name(lang, upper)
        rel, text, imp = konst(lang, name, value)
        parts = [("expr", name)] + ([("lit", tail)] if tail else [])
    else:
        ename, head, tail = enum
        rel, text, imp, expr = route_key(lang, ename)
        parts = ([("lit", head)] if head else []) + [("expr", expr)] + (
            [("lit", tail)] if tail else [])
    o.api[rel] = text
    if imp and imp not in imports:
        imports.append(imp)
    return parts
