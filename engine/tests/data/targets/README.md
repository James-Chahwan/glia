# engine/tests/data/targets

The repo `engine/tests/targets.rs` (HT.6, the build-target answers) builds in a
temp dir: two bazel workspaces of the public repository bazelbuild/examples
(https://github.com/bazelbuild/examples), rev
efab5bd4660ebfe720bd52efaeeccefaf00ee4ad, copied byte for byte with `git
archive`:

- `cpp/` is `cpp-tutorial/stage3` (its `README.md` left out);
- `go/` is `go-tutorial/stage3`.

NOTICE: that repository is licensed under the Apache License, Version 2.0
(https://www.apache.org/licenses/LICENSE-2.0); glia's own licence is separate
and does not apply to these files.

The targets snapshot the test writes beside them is the normalised
`bazel query '//...:*'` output of the same two workspaces that
`snapshots/tests/data/bazel/` holds raw (HT.3) and
`parsers/build/bazel/tests/parse.rs` writes inline (HT.5), as workspaces `cpp`
and `go`; the test writes it inline too, so no test data is shared across
crates. Nothing here runs bazel.

The self-scan skips every `tests/data/` tree, so these sources mint nothing
in glia's own graph.
