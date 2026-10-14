fn main() {
    match bed::app::run_native_fixture() {
        Ok(exit) => std::process::exit(exit),
        Err(error) => {
            eprintln!("bEd native fixture: {error}");
            std::process::exit(1);
        }
    }
}
