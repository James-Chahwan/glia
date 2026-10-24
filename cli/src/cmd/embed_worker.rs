//! `glia embed-worker` (FB.14): the crash-isolated GPU embedding worker, which
//! is this binary. `glia embed` (and every other bge-small surface of the CLI)
//! spawns `glia embed-worker` as the child that holds the WebGPU / CUDA
//! session, so a GPU abort costs only the child: the parent re-embeds the rest
//! on the CPU (`glia_embed::worker`). The protocol is frames on stdin /
//! stdout, never for a person to type.
//!
//! Hidden by construction: it is intercepted in `main` before clap parses, so
//! it is no clap subcommand, appears in neither `glia --help` nor
//! `cli/surface/`, and needs no README `## CLI` line.

/// The argument that makes this binary a worker.
pub(crate) const SUBCOMMAND: &str = "embed-worker";

/// `Some(exit code)` when this process was started as the worker (its first
/// argument is [`SUBCOMMAND`]), after serving the parent; None otherwise.
pub(crate) fn intercept() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|a| a == SUBCOMMAND) {
        return Some(glia_embed::worker::worker_main());
    }
    None
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::SUBCOMMAND;

    /// The worker is no clap command: absent from `glia --help` and from
    /// every committed surface file.
    #[test]
    fn the_worker_is_hidden_from_help_and_the_surface() {
        let mut cmd = crate::Cli::command();
        let help = cmd.render_long_help().to_string();
        assert!(!help.contains(SUBCOMMAND), "{help}");
        assert!(cmd.find_subcommand(SUBCOMMAND).is_none());
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("surface");
        for entry in std::fs::read_dir(&dir).expect("cli/surface") {
            let path = entry.expect("entry").path();
            let body = std::fs::read_to_string(&path).expect("surface file");
            assert!(!body.contains(SUBCOMMAND), "{}", path.display());
        }
    }
}
