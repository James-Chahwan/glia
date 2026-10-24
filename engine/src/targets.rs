//! Build-target graphs (0.5.4, FA.18): the targets and their dependencies read
//! from the `targets sync` snapshot (written from `bazel query` output under
//! `.glia/`), mapped onto the code graph's files. Reads a snapshot only: it
//! never runs bazel and never refreshes the snapshot inside a build.
