//! An HTTP client. `send` makes one request and reads its response, on the
//! calling thread, and can be cancelled while it waits.
mod engine;
pub use engine::*;

#[cfg(test)]
mod tests;
