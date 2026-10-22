use crate::{AnalysisStatus, OutlineNode};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    ops::{ControlFlow, Range},
};
use tree_sitter::{Node, Query, QueryCursor, QueryCursorOptions, StreamingIterator, Tree};
pub(crate) fn query_source(language: &str) -> Option<&'static str> {
    macro_rules! sources { ($($language:literal => $name:literal),* $(,)?) => { match language {
        $($language => Some(include_str!(concat!("queries/", $name, ".scm"))),)*
        _ => None,
    } }; }
    sources!(
        "c" => "c", "cpp" => "cpp", "c-sharp" => "csharp", "go" => "go", "java" => "java",
        "javascript" => "jsx", "typescript" => "tsx", "tsx" => "tsx", "python" => "python",
        "rust" => "rs", "ruby" => "rb", "kotlin" => "kotlin", "bash" => "sh", "json" => "json",
        "toml" => "toml", "hcl" => "hcl", "html" => "html", "css" => "css",
        "markdown" => "markdown",
        "swift" => "swift",
        "dart" => "dart",
        "php" => "php",
        "lua" => "lua",
        "powershell" => "powershell",
        "yaml" => "yaml",
        "xml" => "xml",
        "sql" => "sql",
        "dockerfile" => "dockerfile",
        "cmake" => "cmake",
        "make" => "make",

    )
}

fn compact(text: &[u8]) -> String {
    // Bound labels without cutting UTF-8. Full source ranges remain available for navigation.
    let text = String::from_utf8_lossy(text);
    let mut label = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
        if !label.is_empty() {
            label.push(' ');
            count += 1;
        }
        for ch in word.chars() {
            if count >= 160 {
                label.push('…');
                return label;
            }
            label.push(ch);
            count += 1;
        }
    }
    label
}

fn declarator_name(mut node: Node<'_>) -> Node<'_> {
    // C/C++ declarators wrap identifiers in pointer/array/function declarators.
    while let Some(inner) = node.child_by_field_name("declarator") {
        node = inner;
    }
    node
}

fn header_range(node: Node<'_>, source: &[u8]) -> Range<usize> {
    let range = node.byte_range();
    let end = source[range.clone()]
        .iter()
        .position(|byte| matches!(byte, b'{' | b'\n' | b'\r'))
        .map_or(range.end, |offset| range.start + offset);
    range.start..end
}

fn local_value(node: Node<'_>) -> bool {
    let mut parent = node.parent();
    while let Some(node) = parent {
        if matches!(
            node.kind(),
            "function_item"
                | "function_definition"
                | "function_declaration"
                | "method_declaration"
                | "method_definition"
                | "function_expression"
                | "arrow_function"
                | "method"
                | "singleton_method"
                | "block"
                | "statement_block"
                | "lambda_literal"
        ) {
            return true;
        }
        parent = node.parent();
    }
    false
}

pub(crate) fn extract(
    tree: &Tree,
    query: Option<&Query>,
    language_name: &str,
    text: &str,
    canceled: impl Fn() -> bool,
) -> Result<Vec<OutlineNode>, AnalysisStatus> {
    let Some(query) = query else {
        return Ok(Vec::new());
    };
    let source = text.as_bytes();
    let mut nodes = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(8192);
    let mut progress = |_: &tree_sitter::QueryCursorState| {
        if canceled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let mut matches = cursor.matches_with_options(
        query,
        tree.root_node(),
        source,
        QueryCursorOptions::new().progress_callback(&mut progress),
    );
    let mut entry_indices = HashMap::new();
    let mut markdown_levels = HashMap::new();
    while let Some(found) = matches.next() {
        if canceled() {
            return Err(AnalysisStatus::Pending);
        }
        if nodes.len() > crate::MAX_RESULTS {
            return Err(AnalysisStatus::Limited("Too many outline entries"));
        }
        let name = found
            .captures
            .iter()
            .find(|capture| query.capture_names()[capture.index as usize] == "name")
            .map(|capture| capture.node);
        for capture in found.captures {
            let Some(kind) = query.capture_names()[capture.index as usize].strip_prefix("outline.")
            else {
                continue;
            };
            let node = capture.node;
            if (kind == "constant" || (language_name == "kotlin" && kind == "field"))
                && local_value(node)
            {
                continue;
            }
            if kind == "constant"
                && node.child_by_field_name("value").is_some_and(|value| {
                    matches!(
                        value.kind(),
                        "arrow_function" | "function_expression" | "generator_function"
                    )
                })
            {
                continue;
            }
            let name = name.map(declarator_name);
            let name_range =
                name.map_or_else(|| header_range(node, &source), |name| name.byte_range());
            if name_range.is_empty() {
                continue;
            }
            let label = if matches!(kind, "impl" | "block")
                || (language_name == "css" && node.kind() != "rule_set")
            {
                compact(&source[header_range(node, &source)])
            } else if language_name == "json" && kind == "key" {
                match serde_json::from_slice::<String>(&source[name_range.clone()]) {
                    Ok(name) => {
                        let label = compact(name.as_bytes());
                        if label.is_empty() {
                            compact(&source[name_range.clone()])
                        } else {
                            label
                        }
                    }
                    Err(_) => compact(&source[name_range.clone()]),
                }
            } else if kind == "entry" {
                if !entry_indices.contains_key(&node.id())
                    && let Some(parent) = node.parent()
                {
                    let mut walk = parent.walk();
                    for (index, child) in parent.named_children(&mut walk).enumerate() {
                        if canceled() {
                            return Err(AnalysisStatus::Pending);
                        }
                        entry_indices.insert(child.id(), index);
                    }
                }
                let index = entry_indices.get(&node.id()).copied().unwrap_or(0);
                format!("[{index}]")
            } else if language_name == "html" {
                format!("<{}>", compact(&source[name_range.clone()]))
            } else {
                compact(&source[name_range.clone()])
            };
            if label.is_empty() {
                continue;
            }
            let mut range = node.byte_range();
            if language_name == "markdown" {
                let marker = found
                    .captures
                    .iter()
                    .find(|capture| query.capture_names()[capture.index as usize] == "level")
                    .expect("Markdown outline queries capture a heading level");
                let level = match marker.node.kind() {
                    "atx_h1_marker" | "setext_h1_underline" => 1,
                    "atx_h2_marker" | "setext_h2_underline" => 2,
                    "atx_h3_marker" => 3,
                    "atx_h4_marker" => 4,
                    "atx_h5_marker" => 5,
                    "atx_h6_marker" => 6,
                    _ => unreachable!("Markdown outline queries capture only heading markers"),
                };
                markdown_levels.insert(range.start, level);
            }
            if node.kind() == "file_scoped_namespace_declaration" {
                range.end = tree.root_node().end_byte();
            }
            nodes.push(OutlineNode {
                id: 0,
                label,
                kind: kind.into(),
                range,
                name_range,
                parent: None,
                depth: 0,
            });
        }
    }
    drop(matches);
    if cursor.did_exceed_match_limit() {
        return Err(AnalysisStatus::Limited("Outline query match limit reached"));
    }
    nodes.sort_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then(b.range.end.cmp(&a.range.end))
            .then(a.name_range.start.cmp(&b.name_range.start))
    });
    nodes.dedup_by(|a, b| a.range == b.range && a.name_range == b.name_range);
    if language_name == "markdown" {
        // The grammar nests ATX sections but treats Setext headings as ordinary
        // blocks. Both heading forms end at the next heading of equal or lower level.
        let mut open_sections: Vec<usize> = Vec::new();
        for index in 0..nodes.len() {
            if canceled() {
                return Err(AnalysisStatus::Pending);
            }
            let start = nodes[index].range.start;
            let level = markdown_levels[&start];
            while let Some(&previous) = open_sections.last() {
                if markdown_levels[&nodes[previous].range.start] < level {
                    break;
                }
                nodes[previous].range.end = start;
                open_sections.pop();
            }
            nodes[index].range.end = source.len();
            open_sections.push(index);
        }
    }
    let mut stack: Vec<usize> = Vec::new();
    let mut occurrences = HashMap::new();
    for index in 0..nodes.len() {
        if canceled() {
            return Err(AnalysisStatus::Pending);
        }
        while let Some(&parent) = stack.last() {
            let a = &nodes[parent].range;
            let b = &nodes[index].range;
            if a.start <= b.start && b.end <= a.end && a != b {
                break;
            }
            stack.pop();
        }
        let parent = stack.last().copied();
        let parent_id = parent.map_or(0, |parent| nodes[parent].id);
        let node = &mut nodes[index];
        node.parent = parent;
        node.depth = stack.len();
        let occurrence = occurrences
            .entry((parent_id, node.kind.clone(), node.label.clone()))
            .or_insert(0usize);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (parent_id, &node.kind, &node.label, *occurrence).hash(&mut hasher);
        node.id = hasher.finish();
        *occurrence += 1;
        stack.push(index);
    }
    Ok(nodes)
}
