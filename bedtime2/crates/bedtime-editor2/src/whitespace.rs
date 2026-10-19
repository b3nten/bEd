use crate::{BufferEdit, TextBuffer, TextSelection, text::BufferBuilder};

/// Indentation inserted by Tab and bracket-aware newline. Existing mixed
/// whitespace is preserved; tabs continue to display at four-cell stops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Indentation {
    Tabs,
    TwoSpaces,
    #[default]
    FourSpaces,
    EightSpaces,
}
impl Indentation {
    pub fn unit(self) -> &'static str {
        match self {
            Self::Tabs => "\t",
            Self::TwoSpaces => "  ",
            Self::FourSpaces => "    ",
            Self::EightSpaces => "        ",
        }
    }
    pub fn width(self) -> usize {
        if self == Self::Tabs {
            4
        } else {
            self.unit().len()
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Tabs => "Tabs",
            Self::TwoSpaces => "2 spaces",
            Self::FourSpaces => "4 spaces",
            Self::EightSpaces => "8 spaces",
        }
    }
}

pub(crate) fn leading_end(text: &TextBuffer, row: usize, limit: usize) -> usize {
    text.leading_whitespace_end(row, limit)
}

pub(crate) fn backspace(text: &TextBuffer, head: usize, style: Indentation) -> usize {
    let row = text.line_of(head);
    let start = text.line_start(row);
    if head > start && leading_end(text, row, head) == head {
        let column = text.column(head);
        let target = column.saturating_sub(1) / style.width() * style.width();
        let mut at = text.byte_at_column(row, target);
        if text.column(at) > target {
            at = text.previous_grapheme(at);
        }
        at.min(text.previous_grapheme(head))
    } else {
        text.previous_grapheme(head)
    }
}

pub(crate) fn ending(text: &TextBuffer) -> &'static str {
    let end = text.line_start(1);
    if end > 0 && text.byte(end - 1) == Some(b'\r') {
        "\r"
    } else if end > 1 && text.byte(end - 1) == Some(b'\n') && text.byte(end - 2) == Some(b'\r') {
        "\r\n"
    } else {
        "\n"
    }
}

pub(crate) fn newline(
    text: &TextBuffer,
    selection: TextSelection,
    style: Indentation,
    smart: bool,
) -> (BufferEdit, usize) {
    let mut range = selection.range();
    let end = leading_end(text, text.line_of(range.start), range.start);
    let indent = if smart {
        text.slice(text.line_start(text.line_of(range.start))..end)
    } else {
        TextBuffer::default()
    };
    if smart {
        let row_start = text.line_start(text.line_of(range.start));
        range.start = text.trailing_whitespace_start(row_start..range.start);
        let row_end = text.line_range(text.line_of(range.end)).end;
        // Indexed suffix/prefix summaries keep even huge whitespace-only lines
        // cheap. No complete indentation string is copied to insert a newline.
        let tail = text.slice(range.end..row_end);
        range.end += tail.leading_whitespace_end(0, tail.len());
    }
    let eol = ending(text);
    let previous = if range.start > 0 {
        text.byte(range.start - 1)
    } else {
        None
    };
    let closing = match previous {
        Some(b'{') => Some(b'}'),
        Some(b'[') => Some(b']'),
        Some(b'(') => Some(b')'),
        _ => None,
    };
    let mut replacement = TextBuffer::from(eol).concat(&indent);
    if smart && closing.is_some() {
        replacement = replacement.concat(&TextBuffer::from(style.unit()));
    }
    let head = range.start + replacement.len();
    if smart && closing.is_some() && text.byte(range.end) == closing {
        replacement = replacement.concat(&TextBuffer::from(eol)).concat(&indent);
    }
    (BufferEdit::new(range, replacement), head)
}

/// Tab/Shift+Tab on a selection replace one contiguous span, producing one
/// history entry and keeping the direction and extent of the selection.
pub(crate) fn indent(
    text: &TextBuffer,
    selection: TextSelection,
    style: Indentation,
    outdent: bool,
) -> (BufferEdit, TextSelection) {
    indent_cancellable(
        text,
        selection,
        style,
        outdent,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap()
}
pub(crate) fn indent_cancellable(
    text: &TextBuffer,
    selection: TextSelection,
    style: Indentation,
    outdent: bool,
    cancel: &std::sync::atomic::AtomicBool,
) -> Option<(BufferEdit, TextSelection)> {
    let range = selection.range();
    let first = text.line_of(range.start);
    let last = text.line_of(range.end);
    let last = if range.end > range.start && range.end == text.line_start(last) {
        last.saturating_sub(1)
    } else {
        last
    };
    let start = text.line_start(first);
    let end = text.line_start(last + 1);
    let mut replacement = BufferBuilder::default();
    let mut mapped = selection;
    let mut shift: isize = 0;
    for row in first..=last {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        let at = text.line_start(row);
        let remove = if outdent {
            if text.byte(at) == Some(b'\t') {
                1
            } else {
                (leading_end(text, row, at.saturating_add(style.width())) - at).min(style.width())
            }
        } else {
            0
        };
        let insert = if outdent { "" } else { style.unit() };
        for (old, new) in [
            (selection.anchor(), &mut mapped.anchor),
            (selection.head(), &mut mapped.head),
        ] {
            if old >= at && old <= end {
                if old <= at + remove {
                    *new = (at as isize + shift) as usize + insert.len();
                } else {
                    *new =
                        (old as isize + shift + insert.len() as isize - remove as isize) as usize;
                }
            }
        }
        replacement.push(insert);
        if !replacement.push_range(text, at + remove..text.line_start(row + 1), cancel) {
            return None;
        }
        shift += insert.len() as isize - remove as isize;
    }
    Some((BufferEdit::new(start..end, replacement.finish()), mapped))
}
