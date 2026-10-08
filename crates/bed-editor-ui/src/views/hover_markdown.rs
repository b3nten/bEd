//! Translated from ned editor/views/hover_markdown.{h,cpp}.
//! Fences/rules/prose form the same deliberately small markdown subset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HoverMdBlock {
    pub code: bool,
    pub language: String,
    pub text: String,
}

pub fn split_hover_lines(source: &str) -> Vec<String> {
    source
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect()
}
fn fence_language(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        index += 1;
    }
    if bytes.get(index..index + 3) != Some(b"```") {
        return None;
    }
    index += 3;
    while bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        index += 1;
    }
    let start = index;
    while bytes
        .get(index)
        .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'`')
    {
        index += 1;
    }
    Some(line[start..index].to_ascii_lowercase())
}
pub fn parse_hover_markdown(source: &str) -> Vec<HoverMdBlock> {
    let mut blocks = Vec::new();
    let mut current = HoverMdBlock::default();
    let mut in_fence = false;
    for line in split_hover_lines(source) {
        if let Some(language) = fence_language(&line) {
            if !in_fence {
                if !current.text.is_empty() || current.code {
                    blocks.push(std::mem::take(&mut current));
                }
                current = HoverMdBlock {
                    code: true,
                    language,
                    text: String::new(),
                };
                in_fence = true;
            } else {
                blocks.push(std::mem::take(&mut current));
                in_fence = false;
            }
        } else if in_fence {
            if !current.text.is_empty() {
                current.text.push('\n');
            }
            current.text.push_str(&line);
        } else if line.len() >= 3
            && (line.bytes().all(|byte| byte == b'-') || line.bytes().all(|byte| byte == b'*'))
        {
            if !current.text.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            blocks.push(HoverMdBlock {
                text: "---".into(),
                ..Default::default()
            });
        } else {
            if !current.text.is_empty() {
                current.text.push('\n');
            }
            current.text.push_str(&line);
        }
    }
    if !current.text.is_empty() || current.code {
        blocks.push(current);
    }
    for block in &mut blocks {
        if !block.code && block.text != "---" {
            block.text = block.text.trim_matches([' ', '\t', '\r', '\n']).to_owned();
        }
    }
    blocks.retain(|block| block.code || !block.text.is_empty());
    blocks
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_fenced_code_and_prose() {
        let blocks = parse_hover_markdown("hello `int`\n```cpp\nint x;\n```\nworld");
        assert!(blocks.len() >= 2);
        assert!(
            blocks.iter().any(|block| block.code
                && block.language == "cpp"
                && block.text.contains("int x;"))
        );
        assert!(
            blocks
                .iter()
                .any(|block| !block.code && block.text.contains("hello"))
        );
    }
    #[test]
    fn upstream_rule_block() {
        let blocks = parse_hover_markdown("a\n---\nb");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[1].text, "---");
        assert!(!blocks[1].code);
    }
    #[test]
    fn upstream_adjacent_blank_lines_trim_and_paragraphs_remain() {
        let blocks = parse_hover_markdown(
            "```cpp\nint foo();\n```\n\nFirst paragraph.\n\nSecond paragraph.\n",
        );
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].code);
        assert_eq!(blocks[1].text, "First paragraph.\n\nSecond paragraph.");
    }
    #[test]
    fn upstream_split_terminators_and_cr() {
        assert_eq!(split_hover_lines("a\r\nb\nc"), vec!["a", "b", "c"]);
        assert_eq!(split_hover_lines(""), vec![""]);
        assert_eq!(split_hover_lines("x\n"), vec!["x", ""]);
    }
    #[test]
    fn subset_keeps_original_unclosed_fence_empty_code_and_rule_spelling() {
        let blocks = parse_hover_markdown("  ``` Cpp extra\n\n x\n");
        assert_eq!(
            blocks,
            vec![HoverMdBlock {
                code: true,
                language: "cpp".into(),
                text: " x\n".into()
            }]
        );
        assert_eq!(
            parse_hover_markdown("```\n```"),
            vec![HoverMdBlock {
                code: true,
                ..Default::default()
            }]
        );
        assert_eq!(
            parse_hover_markdown("***\n --- "),
            vec![
                HoverMdBlock {
                    text: "---".into(),
                    ..Default::default()
                },
                HoverMdBlock {
                    text: "---".into(),
                    ..Default::default()
                }
            ]
        );
    }
}
