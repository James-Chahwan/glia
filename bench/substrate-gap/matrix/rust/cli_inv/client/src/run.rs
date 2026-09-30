use std::process::Command;

pub fn run_sync() -> std::io::Result<std::process::ExitStatus> {
    Command::new("mytool").arg("sync").status()
}
