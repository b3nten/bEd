fn main() {
    match bed::app::run() {
        Ok(0) => {}
        Ok(exit) => std::process::exit(exit),
        Err(error) => {
            eprintln!("bEd: {error}");
            std::process::exit(1);
        }
    }
}
