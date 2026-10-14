//! Separate entry point so mock children never load the integration harness.
fn main() -> std::process::ExitCode {
    bedtime_test_fixtures::run()
}
