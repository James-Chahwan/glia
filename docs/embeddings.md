# Embeddings: models, devices, fallbacks

glia embeds a repo's nodes with `glia embed <repo>` (CLI) or `glia_py.embed(repo_path)`
(pyo3). Both write `<repo>/.glia/vectors.jsonl`, the sidecar the semantic tiers read. It is a
snapshot step: no build reads or refreshes it, and vectors are never a build input.

Two models: `bge-small` (fastembed's `BGESmallENV15`, `Xenova/bge-small-en-v1.5`, fp32, CLS
pooling, 384 dims, the 512-token cut) and `test-hash` (a deterministic feature hash for tests,
64 dims, no download). The model downloads once into `FASTEMBED_CACHE_DIR`, else
`$XDG_CACHE_HOME/glia/fastembed`, else `~/.cache/glia/fastembed`; a cache inside the repo being
embedded is refused before anything loads (`glia_embed::refuse_repo_cache`).

## Devices (0.5.4, FB.14)

bge-small runs on the GPU by default. CPU embedding is compute-bound (bge-small at 512 tokens:
about 6-9 texts/s on an 8-core desktop CPU), so a first embed of a 35k-node repo takes the
better part of an hour on the CPU and a few minutes on a mid-range GPU.

| device   | provider                                   | where it runs                    | batch |
|----------|--------------------------------------------|----------------------------------|-------|
| `webgpu` | ONNX Runtime WebGPU (Dawn over Vulkan / Metal / D3D12) | a worker child process | 8 |
| `cuda`   | ONNX Runtime CUDA (opt-in build feature)   | a worker child process           | 32    |
| `cpu`    | ONNX Runtime CPU                           | in the calling process           | 256   |

`GLIA_EMBED_DEVICE` picks the device:

- unset (or empty): WebGPU when the binary (or its worker) carries it, else the CPU;
- `cpu`: the CPU in-process (`fallback=override`);
- `webgpu` / `cuda`: that device, and a device that cannot start is an error naming it (exit 2
  from `glia embed`, a `ValueError` from `glia_py.embed`). CUDA is only ever explicit.

Every embedder prints one line on stderr when its device is settled, and one more if it falls
back later:

```
[embed] device=<adapter name> backend=<cpu|webgpu|cuda> driver=<version> fallback=<reason|none> batch=<n>
```

for example `[embed] device=NVIDIA GeForce GTX 1660 SUPER backend=webgpu driver=580.178.04
fallback=none batch=8`. The adapter is probed through Vulkan (the first discrete GPU, its
driver-info string), else `/proc/driver/nvidia/version`, else `unknown`; for the CPU it is the
CPU's model name and `driver=none`.

### The crash-isolated worker

A GPU session never runs in the calling process. WebGPU on the one card measured aborted the
whole process (`free(): invalid pointer`, a core dump) at batch >= 64, so the session lives in a
worker child that the embedder starts on its first call, reuses for every later call and kills
when it is dropped. The parent sends the texts in frames of at most `batch` texts and gets the
vectors back; if the child dies, the parent re-embeds every text not yet returned on an
in-process CPU session and keeps using it. The CPU path never spawns a child.

- The CLI's worker is the `glia` binary itself (`glia embed-worker`, hidden: no help entry, no
  surface file) when it was built with the WebGPU provider (a source build's default), else the
  `glia-embed-worker` installed beside it.
- glia-py's worker is `glia-embed-worker`, which the wheel installs into the environment's
  scripts directory (`bin/` or `Scripts/`) with the Dawn library; `glia_py.embed` looks for it in
  the interpreter's directory and in the `bin` / `Scripts` directories above the `glia_py`
  module's `__file__` (a `--user` install). The extension itself links only the CPU provider, so
  neither Dawn nor a GPU driver is ever loaded into the Python process.
- Parent and worker shake hands on `glia-embed-worker/1 <release>`: a worker of another release
  is refused (`no_worker:version ..`).

### Fallback reasons

| `fallback=`              | meaning                                                                       |
|--------------------------|-------------------------------------------------------------------------------|
| `none`                   | the requested device runs                                                     |
| `override`               | `GLIA_EMBED_DEVICE=cpu`                                                       |
| `not_built`              | neither the binary nor its worker carries the device (aarch64 Linux, a `--no-default-features --features embed` source build without a worker) |
| `no_worker`              | no worker executable was found (an sdist build of a wheel, a deleted script)  |
| `no_worker:status=127`   | the worker could not start: a loader error, such as a missing Dawn library or a glibc older than 2.38 |
| `no_worker:version ..`   | the worker belongs to another glia release                                    |
| `no_adapter:<error>`     | the GPU session could not commit (no adapter, no Vulkan ICD, no CUDA runtime) |
| `gpu_crash:signal=<n>`   | the worker died mid-run (an abort is `signal=6`)                              |
| `gpu_crash:status=<n>`   | the worker exited non-zero mid-run                                            |
| `oom`                    | CUDA ran out of memory twice (the batch is halved once first)                |
| `error:<message>`        | any other error the worker reported                                           |

After a fallback the run completes on the CPU. ORT's providers are registered with
`error_on_failure`: without it a provider that fails to start would run the graph on the CPU
silently, under the GPU's name.

### Why the batches are pinned

The 2026-10-25 spike (GTX 1660 Super, 6 GB; Dawn over Vulkan; ORT 1.28 through ort
2.0.0-rc.13): WebGPU peaked at batch 8 (30.4 texts/s), its throughput halved above 8 and the
process aborted at batch >= 64; CUDA ran batch 8-64 at 60-66 texts/s and ran out of memory
(a clean error) at 128 and 256; the CPU's throughput does not depend on the batch (8-256). So
WebGPU takes 8, CUDA 32, the CPU 256 texts per session run. Every device's vectors agree with
the CPU's: max abs diff < 4e-7, min cosine 0.99999988, top-10 neighbours 10/10 on 10 queries.

### Rows

Every row glia writes records the device that computed it: `"device":"cpu" | "webgpu" | "cuda"`
(the test model writes none). An absent field reads as `cpu`, so rows written before 0.5.4 read
as before. The device never decides freshness: a row of another device whose text hash matches
is fresh, and a vector reused by text hash keeps its row's device.

## Platform support

| platform            | CLI wheel (`glia-cli`)                 | py wheel (`glia-py`)                    |
|---------------------|----------------------------------------|-----------------------------------------|
| x86_64 Linux        | WebGPU (worker + Dawn beside `glia`), CPU fallback | WebGPU (worker + Dawn in the scripts dir), CPU fallback |
| aarch64 Linux       | CPU (ORT ships no WebGPU build)        | CPU                                     |
| aarch64 macOS       | WebGPU over Metal (ORT's coreml + webgpu build), CPU fallback | same                    |
| x86_64 macOS        | no embeddings (`test-hash` only)       | no embeddings (`test-hash` only)        |
| x86_64 Windows      | WebGPU over D3D12 (worker + Dawn DLLs beside `glia.exe`), CPU fallback | same      |

- The Dawn library ORT links (`libwebgpu_dawn.so`, about 12 MB) needs glibc >= 2.38 on Linux.
  The wheels' `glia` and the py extension do not link it, so they start on every glibc their
  manylinux tag allows; on an older glibc the worker cannot load (`no_worker:status=127`) and
  embedding runs on the CPU.
- x86_64 macOS: no ONNX Runtime >= 1.24 build exists for x86_64-apple-darwin (ort 2.0.0-rc.12,
  rc.13 and rc.15 ship none, and Microsoft stopped onnxruntime-osx-x86_64 after 1.23.2), so
  bge-small refuses there with "embeddings are not available on Intel macOS"; the wheels keep
  their macOS 10.12 tag.
- CUDA is never in a published wheel (the provider plus the user's CUDA runtime and cuDNN run to
  hundreds of MB). Build it from source: `cargo build --release -p glia-cli --features gpu-cuda`
  (ort 2.0.0-rc.13 ships CUDA 13 builds only: the CUDA 13 runtime and cuDNN 9 must be on the
  library path), then `GLIA_EMBED_DEVICE=cuda glia embed <repo>`.
- A source build of the CLI carries WebGPU by default (`gpu-webgpu`), and the binary then links
  Dawn (an `$ORIGIN` / `@loader_path` rpath finds the copy ORT's build puts beside it in
  `target/<profile>/`). On a glibc older than 2.38 build `--no-default-features --features
  embed` (the CPU), or add a `glia-embed-worker`:
  `cargo build --release -p glia-embed --features gpu-webgpu --bin glia-embed-worker`.

### How the wheels bundle the worker

`cli/pyproject.toml` builds `glia` with `embed` only (no Dawn). The wheels workflows build
glia-embed's `glia-embed-worker` with `gpu-webgpu` on the platforms above, copy it and the Dawn
library (`libwebgpu_dawn.so` / `.dylib`, every `.dll` ORT's Windows build ships) into
`cli/glia_cli.data/scripts/` (`py/glia_py.data/scripts/`) and append `data = "glia_cli.data"`
(`"glia_py.data"`) to the pyproject in the job's checkout, so maturin puts them in the wheel's
`.data/scripts/` and pip installs them beside the interpreter's scripts. The worker's rpath is
`$ORIGIN` / `@loader_path` (embed/build.rs), so it finds Dawn beside itself; Windows loads a DLL
from the executable's directory. This is the mechanism the Linux smoke tests verify:
`scripts/check-cli-wheel.sh` installs the built `glia-cli` wheel into a clean venv and runs
`glia embed` with the test model, then bge-small with `VK_DRIVER_FILES=/nonexistent.json`
(`fallback=no_adapter:..`, completed on the CPU) and on the host's devices. maturin's own
`auditwheel` repair is NOT used for Dawn: for a bin wheel it grafts the library into
`glia_cli.libs/` with an rpath (`$ORIGIN/../glia_cli.libs`) that points nowhere once pip has
put the binary in `bin/`. A fixed `data` key is not committed because maturin fails a build
whose data directory is missing (the sdist's): an sdist build ships no worker and embeds on the
CPU (`fallback=no_worker` / `not_built`).

## Measured

The spike's 256 texts (consecutive 4000-byte chunks of `engine/src/*.rs`), one process at a time
on an otherwise idle machine (Ryzen 7 3700X, 8 cores / 16 threads; GTX 1660 Super 6 GB, NVIDIA
580.178.04), bge-small, after one warm-up batch:

| route (harness)                                    | batch | texts/s | vs CPU | max abs diff vs fastembed 4.9.1 CPU | top-10 (10 queries) |
|----------------------------------------------------|-------|---------|--------|-------------------------------------|---------------------|
| fastembed 4.9.1 CPU (ORT 1.20, the 0.5.4 baseline) | 256   | 9.77    | 1.0x   | 0                                   | 10/10               |
| ort 2.0.0-rc.13 CPU (ORT 1.28)                     | 256   | 9.61    | 1.0x   | 2.98e-7                             | 10/10               |
| ort 2.0.0-rc.13 WebGPU (Dawn over Vulkan)          | 8     | 48.91   | 5.0x   | 3.87e-7                             | 10/10               |
| fastembed 4.9.1 + ort CUDA (CUDA 12, cuDNN 9)      | 32    | 85.79   | 8.8x   | 3.54e-7                             | 10/10               |

Min cosine against the baseline is 0.99999988 on every route. Measured 2026-10-25 with the
spike harness (`ort-raw`, `fe-bench`, `compare.py`), the CPU numbers at batch 256 since the CPU's
throughput does not depend on the batch. The 0.5.4 ORT move itself (ORT 1.20 -> 1.28 on the
CPU) moves a vector by at most 3e-7 and changes no ranking.

End to end, `glia embed` (a debug build, recipe t1) over a copy of glia's `graph/` crate, 2661
nodes: WebGPU 18.1 s (147 texts/s), `GLIA_EMBED_DEVICE=cpu` 118.7 s (22.4 texts/s), 6.5x; the
two sidecars agree to 4.2e-7 per component. A full CPU embed of glia's 36,133 nodes took 37:43
on a shared machine (FB.5b), the second run 2.9 s (every row fresh).

## Reporting a device problem

Open an issue with:

1. the `[embed] device=.. backend=.. driver=.. fallback=.. batch=..` line(s) from stderr;
2. the OS and version, the GPU, and how glia was installed (wheel, source build and features);
3. whether `GLIA_EMBED_DEVICE=cpu` completes the same run;
4. for a crash, the worker's last stderr lines (they are forwarded with the parent's).

An adapter that commits but computes wrong vectors is caught only this way: the parity test
(`cargo test -p glia-embed --features gpu-webgpu --test device -- --ignored`) compares a device
with the CPU (max abs diff < 1e-5, top-10 neighbours 10/10) on the spike's 256 texts.
