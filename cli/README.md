# glia-cli

glia parses a repository into one cross-language code graph (calls, imports, the HTTP / gRPC / GraphQL / queue / event / WebSocket links between services, data access, tests and docs) and answers structural questions from it: where a symbol is, what a change affects, how a feature flows end to end, which tests cover a diff. Each answer is one `glia <command> <repo>` call, ranked and located (`file:line`), with `--json` for structured output. This package is the `glia` command-line tool as a prebuilt binary: no Rust toolchain, no server.

## Install

```
# run once, nothing installed
uvx --from glia-cli glia --version

# install the command
pipx install glia-cli

# or into the current environment (puts `glia` on PATH)
pip install glia-cli
```

## Use

```
glia find <repo> HashPassword --json          # where is X?
glia blast-radius <repo> HashPassword --json  # what does changing X affect?
glia trace <repo> login --json                # how does feature Y flow, end to end?
glia diff-impact <repo> --json                # what does my uncommitted change affect?
glia review <repo> --base main --json         # my change as one PR report
```

Without installing: `uvx --from glia-cli glia find <repo> HashPassword --json`.

`glia --help` lists every command, `glia <command> --help` every flag. The full command reference is in the [glia README](https://github.com/James-Chahwan/glia#cli).

## Agents

The [skills](https://github.com/James-Chahwan/glia/tree/main/skills) teach an agent (Claude Code) to answer structural questions with this CLI: which command fits which question, and how to read an answer's locations, tiers and absences.

## Library

For Python scripts, the same engine ships as the [`glia-py`](https://pypi.org/project/glia-py/) wheel (`import glia_py`).

## License

[Glia Software License v0.1](https://github.com/James-Chahwan/glia/blob/main/LICENSE): PolyForm Noncommercial 1.0.0 with Additional Permissions for worker-protective commercial use. Commercial license required otherwise; see the [License section](https://github.com/James-Chahwan/glia#license).
