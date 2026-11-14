//! What Structure, the breadcrumbs and Go to Symbol share about outline
//! entries: their kinds' icons and colors, where the caret is among them,
//! fuzzy matching, and the text after a name.
use crate::syntax::{Observation, ObservationKey, OutlineNode};
use bedtime_ui::{BuiltinIcon, theme::Rgba};
use std::{ops::Range, sync::Arc};

/// The outline queries' kinds, in the groups Structure shows (and can hide).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Function,
    Type,
    Interface,
    Field,
    Constant,
    Variant,
    Module,
    Macro,
    Section,
    Element,
    Other,
}

impl Category {
    pub const ALL: [Category; 11] = [
        Category::Function,
        Category::Type,
        Category::Interface,
        Category::Field,
        Category::Constant,
        Category::Variant,
        Category::Module,
        Category::Macro,
        Category::Section,
        Category::Element,
        Category::Other,
    ];

    pub fn of(kind: &str) -> Self {
        match kind {
            "function" | "method" => Category::Function,
            "type" | "class" | "table" => Category::Type,
            "interface" | "trait" | "impl" => Category::Interface,
            "field" | "variable" | "key" => Category::Field,
            "constant" => Category::Constant,
            "variant" => Category::Variant,
            "module" | "namespace" => Category::Module,
            "macro" => Category::Macro,
            "section" => Category::Section,
            "element" => Category::Element,
            _ => Category::Other,
        }
    }

    /// A bit of `StructurePanel::hidden`.
    pub fn bit(self) -> u16 {
        1 << self as u16
    }

    /// The name in Structure's Show menu.
    pub fn label(self) -> &'static str {
        match self {
            Category::Function => "Functions and methods",
            Category::Type => "Types and classes",
            Category::Interface => "Traits, interfaces and impls",
            Category::Field => "Fields and variables",
            Category::Constant => "Constants",
            Category::Variant => "Variants",
            Category::Module => "Modules and namespaces",
            Category::Macro => "Macros",
            Category::Section => "Headings",
            Category::Element => "Elements",
            Category::Other => "Other",
        }
    }

    pub fn icon(self) -> BuiltinIcon {
        match self {
            Category::Function => BuiltinIcon::SymbolFunction,
            Category::Type => BuiltinIcon::Box,
            Category::Interface => BuiltinIcon::SymbolInterface,
            Category::Field | Category::Constant => BuiltinIcon::SymbolVariable,
            Category::Variant => BuiltinIcon::SymbolVariant,
            Category::Module => BuiltinIcon::SymbolModule,
            Category::Macro => BuiltinIcon::SymbolMacro,
            Category::Section => BuiltinIcon::SymbolSection,
            Category::Element => BuiltinIcon::SymbolElement,
            Category::Other => BuiltinIcon::SymbolRule,
        }
    }

    /// The icon's ink: the color the theme gives such names in code.
    pub fn ink(self, syntax: &bedtime_ui::theme::Syntax) -> Rgba {
        match self {
            Category::Function | Category::Macro => syntax.function,
            Category::Type | Category::Interface => syntax.r#type,
            Category::Field => syntax.property,
            Category::Constant | Category::Variant => syntax.constant,
            Category::Module => syntax.keyword,
            Category::Section | Category::Element => syntax.keyword,
            Category::Other => syntax.special,
        }
    }
}

/// The innermost entry around `byte`: its index.
pub fn entry_at(outline: &[OutlineNode], byte: usize) -> Option<usize> {
    // Parents precede children, so the last one containing it is innermost.
    // An entry's end counts: a caret just after `}` is still in it.
    outline
        .iter()
        .rposition(|node| node.range.start <= byte && byte <= node.range.end)
}

/// `index` and its ancestors, innermost first.
pub fn ancestry(outline: &[OutlineNode], index: usize) -> impl Iterator<Item = usize> + '_ {
    std::iter::successors(Some(index), |&at| outline[at].parent)
}

/// What follows an entry's name on its first line (parameters, a type),
/// and the line it starts on, from the analysed text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Detail {
    pub text: String,
    /// Zero-based.
    pub line: usize,
}

/// Each entry's `Detail`, worked out once per analysis.
#[derive(Clone, Default)]
pub struct Details(Option<(ObservationKey, Arc<Vec<Detail>>)>);

impl Details {
    pub fn of(&mut self, observation: &Observation) -> Arc<Vec<Detail>> {
        if let Some((key, details)) = &self.0
            && *key == observation.key
        {
            return details.clone();
        }
        let details = Arc::new(details(observation));
        self.0 = Some((observation.key, details.clone()));
        details
    }
}

const MAX_DETAIL: usize = 80;

fn details(observation: &Observation) -> Vec<Detail> {
    let source = &observation.source;
    let mut details = Vec::with_capacity(observation.outline.len());
    // Lines, counted once through the text: entries are in order of start.
    let mut counted = 0;
    let mut line = 0;
    for node in &observation.outline {
        let start = node.range.start.min(source.len());
        if start >= counted {
            line += count_newlines(source.as_ref(), counted..start);
            counted = start;
        } else {
            // Out of order (shouldn't be): count from the top.
            line = count_newlines(source.as_ref(), 0..start);
            counted = start;
        }
        let text = if wants_detail(&node.kind) {
            detail_after(source.as_ref(), node.name_range.end, node.range.end)
        } else {
            String::new()
        };
        details.push(Detail { text, line });
    }
    details
}

fn wants_detail(kind: &str) -> bool {
    matches!(
        Category::of(kind),
        Category::Function
            | Category::Type
            | Category::Interface
            | Category::Field
            | Category::Constant
            | Category::Variant
            | Category::Macro
    ) && kind != "impl"
        && kind != "key"
}

fn count_newlines(source: &dyn crate::syntax::Source, range: Range<usize>) -> usize {
    let mut count = 0;
    let mut at = range.start;
    while at < range.end {
        let chunk = source.chunk(at);
        if chunk.is_empty() {
            break;
        }
        let take = chunk.len().min(range.end - at);
        count += chunk[..take].iter().filter(|&&b| b == b'\n').count();
        at += take;
    }
    count
}

/// The rest of the line after a name, up to a body (`{`, `=`, `:` that
/// starts a Python block, `where`), tidied: `(a: u32) -> bool`, `String`.
fn detail_after(source: &dyn crate::syntax::Source, from: usize, end: usize) -> String {
    let mut bytes = Vec::new();
    let mut at = from;
    let end = end.min(source.len());
    'read: while at < end && bytes.len() < 400 {
        let chunk = source.chunk(at);
        if chunk.is_empty() {
            break;
        }
        for &byte in &chunk[..chunk.len().min(end - at)] {
            if byte == b'\n' {
                break 'read;
            }
            bytes.push(byte);
        }
        at += chunk.len();
    }
    let line = String::from_utf8_lossy(&bytes);
    let mut cut = line.len();
    let mut depth = 0i32;
    for (index, c) in line.char_indices() {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' => depth -= 1,
            '>' if !line[..index].ends_with('-') && !line[..index].ends_with('=') => depth -= 1,
            '{' | '=' if depth <= 0 => {
                cut = index;
                break;
            }
            _ => {}
        }
    }
    let mut text = line[..cut].trim().to_owned();
    if let Some(stripped) = text.strip_suffix(" where") {
        text = stripped.trim_end().to_owned();
    }
    let text = text
        .trim_end_matches([',', ';'])
        .trim_end_matches(':')
        .trim_end();
    // A field's or constant's type, without the colon: `age  u32`.
    let text = text.strip_prefix(':').map_or(text, str::trim_start);
    let compact: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > MAX_DETAIL {
        let mut cut: String = compact.chars().take(MAX_DETAIL - 1).collect();
        cut.push('…');
        cut
    } else {
        compact
    }
}

/// The separator between names in a path to `language`'s entries.
pub fn path_separator(language: &str) -> &'static str {
    match language {
        "rust" | "cpp" | "c" | "php" => "::",
        "markdown" => " › ",
        _ => ".",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_follow_names() {
        let detail = |line: &str| {
            let source = line.to_owned();
            detail_after(&source, 0, source.len())
        };
        assert_eq!(detail("(a: u32) -> bool {"), "(a: u32) -> bool");
        assert_eq!(detail(": Vec<String>,"), "Vec<String>");
        assert_eq!(
            detail("<T: Clone> where T: Send {"),
            "<T: Clone> where T: Send"
        );
        assert_eq!(detail(" = 42;"), "");
        assert_eq!(detail("(x = 1, y = 2) {"), "(x = 1, y = 2)");
        assert_eq!(detail("(self):"), "(self)");
    }

    #[test]
    fn the_innermost_entry_holds_the_caret() {
        let node = |range: Range<usize>, parent| OutlineNode {
            id: range.start as u64,
            label: String::new(),
            kind: "function".into(),
            name_range: range.start..range.start + 1,
            range,
            parent,
            depth: parent.map_or(0, |_| 1),
        };
        let outline = vec![node(0..50, None), node(10..20, Some(0)), node(60..70, None)];
        assert_eq!(entry_at(&outline, 15), Some(1));
        assert_eq!(entry_at(&outline, 30), Some(0));
        assert_eq!(entry_at(&outline, 55), None);
        assert_eq!(ancestry(&outline, 1).collect::<Vec<_>>(), vec![1, 0]);
    }
}
