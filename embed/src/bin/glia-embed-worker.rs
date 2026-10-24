//! `glia-embed-worker`: the crash-isolated embedding worker for hosts that are
//! not the `glia` binary (glia-py's wheel installs it beside the interpreter's
//! scripts). The protocol and everything else live in `glia_embed::worker`.

fn main() {
    std::process::exit(glia_embed::worker::worker_main());
}
