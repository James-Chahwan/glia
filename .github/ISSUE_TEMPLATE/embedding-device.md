---
name: Embedding device problem
about: glia embed (or glia_py.embed) is slow, falls back to the CPU, or fails on your GPU
labels: embeddings
---

docs/embeddings.md lists the devices, the fallbacks and what each platform supports (no
embeddings on Intel macOS; CPU only on aarch64 Linux; the WebGPU worker needs glibc >= 2.38 on
Linux; CUDA only from a source build, CUDA 13).

**The `[embed] device=` line**

Every embed run prints one on stderr, and one more if it falls back. Paste both:

```
[embed] device=.. backend=.. driver=.. fallback=.. batch=..
```

**Versions**

- `glia --version` (or `glia_py.version()` and `glia_py.build_stamp()`):
- OS, version and architecture:
- GPU model and driver version:
- Installed from: `glia-cli` / `glia-py` wheel, `uvx --from glia-cli`, or a source build (commit and `--features`):
- `GLIA_EMBED_DEVICE`, if set:

**The command**

```
glia embed <repo> ...
```

How many nodes it embedded, how long it took (the `[embed] repo=.. ms=..` line), and any error
text.
