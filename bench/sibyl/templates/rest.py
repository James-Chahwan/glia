"""The REST transport: places a case's client and server templates by `packaging`, writes the
manifests, and derives the expected answer from the renders themselves.

`render(case, state)` -> {repo name: {path: text}} for state P (both sides read the old field),
S (the server renamed it; the client is stale) or C (the client followed). `expected(case)` ->
the answer glia must give, every file in it read off the renders (the drift's file sets are the
P->S and S->C differences, never written by hand)."""

import json

from . import rest_clients, rest_servers
from .common import NEW, OLD, PATH, client_field, norm_path, server_field

STATES = ("P", "S", "C")
STATE_NOTES = {
    "P": "base: client and server agree on " + OLD.camel,
    "S": f"the server renames {OLD.camel} to {NEW.camel}; the client is stale",
    "C": f"the client follows: it reads {NEW.camel}",
}


def layout(packaging):
    """(client repo, client base, api base, app base, server repo, server base)."""
    if packaging == "workspace":
        return "repo", "", "packages/api-client", "apps/web", "repo", "services/api"
    if packaging == "multi-repo":
        return "client", "", "", "", "server", ""
    return "repo", "client", "client", "client", "repo", "server"


def _j(*parts):
    return "/".join(p.strip("/") for p in parts if p)


def _gradle(deps, serialization, extra="", library=False):
    """A Kotlin build script; a workspace library exposes its deps with `api(..)`, since the app
    reads types they define (a JSONObject, a Retrofit Call)."""
    plugins = ["    `java-library`"] if library else []
    plugins.append('    kotlin("jvm") version "2.0.21"')
    if serialization:
        plugins.append('    kotlin("plugin.serialization") version "2.0.21"')
    conf = "api" if library else "implementation"
    lines = ["plugins {", *plugins, "}", "", "repositories {", "    mavenCentral()", "}", "",
             "dependencies {"]
    lines += [f'    {conf}("{rest_clients.DEP_VERSIONS[d]}")' for d in deps]
    if extra:
        lines.append(extra)
    lines.append("}")
    return "\n".join(lines) + "\n"


def _swift_package(name, dep=None):
    deps, tdeps = "", ""
    if dep:
        deps = f'    dependencies: [.package(path: "{dep[0]}")],\n'
        tdeps = f', dependencies: [.product(name: "{dep[1]}", package: "{dep[2]}")]'
    return (
        "// swift-tools-version:5.9\nimport PackageDescription\n\n"
        "let package = Package(\n"
        f'    name: "{name}",\n'
        "    platforms: [.macOS(.v12), .iOS(.v15)],\n"
        f'    products: [.library(name: "{name}", targets: ["{name}"])],\n'
        f"{deps}"
        f'    targets: [.target(name: "{name}"{tdeps})]\n'
        ")\n"
    )


def _pubspec(name, deps, dev=(), path_deps=(), workspace_member=False):
    lines = [f"name: {name}", "publish_to: none", "", "environment:", "  sdk: ^3.6.0"]
    if workspace_member:
        lines.append("resolution: workspace")
    lines += ["", "dependencies:"]
    lines += [f"  {d}: {rest_clients.DEP_VERSIONS[d]}" for d in deps]
    lines += [f"  {n}:\n    path: {p}" for n, p in path_deps]
    if dev:
        lines += ["", "dev_dependencies:"] + [f"  {d}: {v}" for d, v in dev]
    return "\n".join(lines) + "\n"


def _pyproject(name, deps, extra=""):
    dl = ", ".join(json.dumps(rest_clients.DEP_VERSIONS.get(d, d)) for d in deps)
    return (f'[project]\nname = "{name}"\nversion = "0.1.0"\nrequires-python = ">=3.11"\n'
            f"dependencies = [{dl}]\n{extra}")


def place_client(ax, out, files, pack, ws_eco):
    """Write the client side into `files` (repo-relative). Returns (call file, reader file).
    `ws_eco` is the server's ecosystem when it shares the client's workspace manifest."""
    lang = ax["client"]
    _, cb, api, app, _, sb = layout(ax["packaging"])
    workspace = pack == "workspace"
    schema = ax["contract"] == "schema-object"

    def put(base, rel, text):
        files[_j(base, rel)] = text

    if lang == "ts":
        api_src, app_src = (_j(api, "src"), _j(app, "src")) if workspace else (
            _j(cb, "src/api"), _j(cb, "src"))
        tsconfig = rest_servers.TSCONFIG
        if workspace:
            globs = ["apps/*", "packages/*"] + (["services/*"] if ws_eco == "node" else [])
            put("", "package.json", rest_clients.dumps({"name": "sibyl", "private": True}))
            put("", "pnpm-workspace.yaml",
                "packages:\n" + "".join(f"  - '{g}'\n" for g in globs))
            put(api, "package.json", rest_clients.dumps({
                "name": "@sibyl/api-client", "version": "0.1.0", "private": True,
                "main": "src/index.ts", "types": "src/index.ts",
                "dependencies": {d: rest_clients.DEP_VERSIONS[d] for d in out.deps}}))
            put(api, "tsconfig.json", tsconfig)
            put(api_src, "index.ts", "".join(f"export * from './{m}';\n" for m in out.exports))
            put(app, "package.json", rest_clients.dumps({
                "name": "@sibyl/web", "version": "0.1.0", "private": True,
                "dependencies": {"@sibyl/api-client": "workspace:*"}}))
            put(app, "tsconfig.json", tsconfig)
        else:
            put(cb, "package.json", rest_clients.dumps({
                "name": "sibyl-web", "version": "0.1.0", "private": True,
                "dependencies": {d: rest_clients.DEP_VERSIONS[d] for d in out.deps}}))
            put(cb, "tsconfig.json", tsconfig)
    elif lang == "dart":
        dev = [("build_runner", "^2.4.13"), ("json_serializable", "^6.9.0")] if schema else []
        if workspace:
            pkg = api.replace("api-client", "api_client")  # a pub package dir is its name
            api_src, app_src = _j(pkg, "lib/src"), _j(app, "lib")
            put("", "pubspec.yaml", "name: sibyl_workspace\npublish_to: none\n\nenvironment:\n"
                f"  sdk: ^3.6.0\n\nworkspace:\n  - {app}\n  - {pkg}\n")
            put(pkg, "pubspec.yaml", _pubspec("api_client", out.deps, dev, workspace_member=True))
            put(pkg, "lib/api_client.dart",
                "".join(f"export 'src/{m}';\n" for m in out.exports))
            put(app, "pubspec.yaml", _pubspec("sibyl_web", [], path_deps=[
                ("api_client", "../../packages/api_client")], workspace_member=True))
        else:
            api_src, app_src = _j(cb, "lib/api"), _j(cb, "lib")
            put(cb, "pubspec.yaml", _pubspec("sibyl_client", out.deps, dev))
    elif lang == "kotlin":
        ser = "kotlinx-serialization-json" in out.deps
        if workspace:
            api_src, app_src = _j(api, "src/main/kotlin"), _j(app, "src/main/kotlin")
            inc = ['"apps:web"', '"packages:api-client"'] + (
                ['"services:api"'] if ws_eco == "gradle" else [])
            put("", "settings.gradle.kts",
                f'rootProject.name = "sibyl"\n\ninclude({", ".join(inc)})\n')
            put(api, "build.gradle.kts", _gradle(out.deps, ser, library=True))
            put(app, "build.gradle.kts",
                _gradle([], False, '    implementation(project(":packages:api-client"))'))
        else:
            api_src = app_src = _j(cb, "src/main/kotlin")
            put(cb, "settings.gradle.kts", 'rootProject.name = "sibyl-client"\n')
            put(cb, "build.gradle.kts", _gradle(out.deps, ser))
    elif lang == "swift":
        if workspace:
            api_src, app_src = _j(api, "Sources/ApiClient"), _j(app, "Sources/Web")
            put(api, "Package.swift", _swift_package("ApiClient"))
            put(app, "Package.swift", _swift_package(
                "Web", ("../../packages/api-client", "ApiClient", "api-client")))
        else:
            api_src, app_src = _j(cb, "Sources/Client/API"), _j(cb, "Sources/Client")
            put(cb, "Package.swift", _swift_package("Client"))
    elif lang == "python":
        if workspace:
            api_src = _j(api, "src/sibyl_api_client")
            app_src = _j(app, "src/sibyl_web")
            members = ["apps/*", "packages/*"] + (["services/*"] if ws_eco == "python" else [])
            put("", "pyproject.toml", '[project]\nname = "sibyl"\nversion = "0.1.0"\n'
                'requires-python = ">=3.11"\n\n[tool.uv.workspace]\n'
                f"members = {json.dumps(members)}\n")
            put(api, "pyproject.toml", _pyproject("sibyl-api-client", out.deps))
            put(app, "pyproject.toml", _pyproject(
                "sibyl-web", ["sibyl-api-client"],
                "\n[tool.uv.sources]\nsibyl-api-client = { workspace = true }\n"))
            put(app_src, "__init__.py", "")
        else:
            api_src, app_src = _j(cb, "sibyl_client/api"), _j(cb, "sibyl_client")
            put(cb, "pyproject.toml", _pyproject("sibyl-client", out.deps))
            put(app_src, "__init__.py", "")
        put(api_src, "__init__.py", "")
    else:
        raise KeyError(f"no REST client template for {lang!r}")
    for rel, text in out.api.items():
        put(api_src, rel, text)
    for rel, text in out.app.items():
        put(app_src, rel, text)
    return _j(api_src, out.call), _j(app_src, out.reader)


ECO_OF_CLIENT = {"ts": "node", "dart": "dart", "kotlin": "gradle", "swift": "swift",
                 "python": "python"}


def _render(ax, state):
    crepo, _, _, _, srepo, sb = layout(ax["packaging"])
    pack = "workspace" if ax["packaging"] == "workspace" else "single"
    sfiles, smeta = rest_servers.SERVERS[ax["server"]](ax["wiring_source"], server_field(state))
    shared = pack == "workspace" and smeta["eco"] == ECO_OF_CLIENT[ax["client"]]
    repos = {crepo: {}, srepo: {}}
    out = rest_clients.CLIENTS[ax["client"]](ax, client_field(state), pack)
    call, reader = place_client(ax, out, repos[crepo], pack, smeta["eco"] if shared else None)
    for rel, text in sfiles.items():
        repos[srepo][_j(sb, rel)] = text
    if "settings" in smeta and not shared:
        repos[srepo][_j(sb, "settings.gradle.kts")] = smeta["settings"]
    meta = {
        "client": {"repo": crepo, "file": call},
        "reader": {"repo": crepo, "file": reader},
        "server": {"repo": srepo, "route_file": _j(sb, smeta["route_file"]),
                   "handler": smeta["handler"], "handler_file": _j(sb, smeta["handler_file"]),
                   "model_file": _j(sb, smeta["model_file"])},
    }
    return {r: dict(sorted(fs.items())) for r, fs in sorted(repos.items())}, meta


def render(case, state):
    return _render(case["axes"], state)[0]


def _changed(a, b):
    out = []
    for repo in sorted(set(a) | set(b)):
        fa, fb = a.get(repo, {}), b.get(repo, {})
        for path in sorted(set(fa) | set(fb)):
            if fa.get(path) != fb.get(path):
                out.append({"repo": repo, "file": path})
    return out


def expected(case):
    ax = case["axes"]
    renders = {s: _render(ax, s)[0] for s in STATES}
    meta = _render(ax, "S")[1]
    repos = sorted(renders["P"])
    multi = ax["packaging"] == "multi-repo"
    return {
        "id": case["id"],
        "transport": "rest",
        "axes": ax,
        "repos": repos,
        # The repo whose change starts the drift is built; the others join with --with.
        "primary": "server" if multi else "repo",
        "with": ["client"] if multi else [],
        "exchange": {"method": "GET", "path": PATH, "path_norm": norm_path(PATH)},
        "pair": {"category": "HTTP_CALLS", "client": meta["client"], "server": meta["server"]},
        "reader": meta["reader"],
        "drift": {
            "field": {"old": OLD.camel, "new": NEW.camel},
            "direction": "response",
            "server_files": _changed(renders["P"], renders["S"]),
            "client_files": _changed(renders["S"], renders["C"]),
            "flag_at": "S",
            "clear_at": "C",
        },
        "states": STATE_NOTES,
    }
