use std::{path::PathBuf, process::Command};
pub fn executable() -> PathBuf {
    env!("CARGO_BIN_EXE_bed-test-fixture").into()
}
pub fn command(mode: &str) -> Command {
    let mut command = Command::new(executable());
    command.env("BED_TEST_FIXTURE_MODE", mode);
    if mode != "navigation" {
        command.arg("--stdio");
    }
    command
}
