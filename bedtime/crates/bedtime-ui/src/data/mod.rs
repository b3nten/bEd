//! Data (feature `data`): JSON and TOML parsed into one `Value` tree
//! (`parse`), and `DataTree` drawing it as rows that open and close.
mod tree;
mod value;
pub use tree::DataTree;
pub use value::{Format, Key, Kind, Value, parse, path_text};
