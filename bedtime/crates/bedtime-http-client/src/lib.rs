//! An HTTP client. `send` makes one request and reads its response, on the
//! calling thread, and can be cancelled while it waits. `Collection` holds
//! saved requests and the variables they use. With the `workbench` feature,
//! `HttpClientPanel` edits a workspace's requests, sends them and shows the
//! responses.
mod collection;
mod engine;
mod json;
#[cfg(feature = "workbench")]
mod panel;

pub use collection::*;
pub use engine::*;
pub use json::pretty as pretty_json;
#[cfg(feature = "workbench")]
pub use panel::{
    Exchange, Exchanges, HttpClientPanel, HttpClientPlugin, HttpClientSystems, HttpClientView,
    HttpRequests, Received, ShowHttpClient, Shown,
};

#[cfg(test)]
mod tests;
