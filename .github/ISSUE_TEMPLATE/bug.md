---
name: Bug report
about: A wrong, missing or crashing answer, or a build that fails
labels: bug
---

**What you ran**

The exact command (or the pyo3 call), and the repo it ran on if it is public (a link and the
commit). For a private repo, describe its shape (languages, frameworks, one repo or several)
instead of pasting its code.

```
glia <command> <repo> ...
```

**What happened, and what you expected**

The answer (or the error) and the row you expected to see or not to see, with its `file:line`.

**Versions**

- `glia --version` (or `python -c "import glia_py; print(glia_py.version(), glia_py.build_stamp())"`):
- OS and architecture:
- Installed from: `glia-cli` / `glia-py` wheel, `uvx --from glia-cli`, or a source build (commit):

**stderr**

The lines glia printed on stderr (they start with `[<name>]`), at least the ones about the part
that went wrong. `glia health <repo>` output helps when an answer is empty.
