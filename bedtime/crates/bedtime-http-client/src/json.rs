//! Showing a JSON body indented. Re-indents the text itself rather than
//! parsing to values and back, so keys keep their order and numbers their
//! digits.

/// `body` indented two spaces a level, if it's JSON.
pub fn pretty(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde::de::IgnoredAny>(body).ok()?;
    let text = std::str::from_utf8(body).ok()?;
    Some(reindent(text))
}

/// Indent valid JSON `text`. Whitespace between tokens is replaced; strings
/// are copied as they are.
fn reindent(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut depth = 0usize;
    let mut chars = text.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n("  ", depth));
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push('"');
                while let Some(c) = chars.next() {
                    out.push(c);
                    match c {
                        '\\' => out.extend(chars.next()),
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '{' | '[' => {
                out.push(c);
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                // Empty: `{}`, `[]` on one line.
                if matches!(chars.peek(), Some('}' | ']')) {
                    out.extend(chars.next());
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(',');
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    out
}
