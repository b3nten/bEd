use std::{path::PathBuf, process::Command};
pub fn executable() -> PathBuf {
    env!("CARGO_BIN_EXE_bedtime-test-fixture").into()
}
pub fn command(mode: &str) -> Command {
    let path = if mode == "navigation" {
        PathBuf::from(env!("CARGO_BIN_EXE_bed-navigation-fixture"))
    } else {
        executable()
    };
    let mut command = Command::new(path);
    command.env("BED_TEST_FIXTURE_MODE", mode);
    if mode != "navigation" {
        command.arg("--stdio");
    }
    command
}
