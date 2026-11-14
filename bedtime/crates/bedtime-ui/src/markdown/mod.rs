//! Markdown (feature `markdown`): `parse` reads it into blocks and runs, and
//! `MarkdownView` draws them, as a document or compact (tooltips).
mod parse;
mod view;
pub use parse::{Block, Callout, Code, Item, Markdown, Run, Style, parse, slug};
pub use view::{MarkdownStyle, MarkdownView, percent_decode, source_path};
