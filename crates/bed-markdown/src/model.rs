use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::collections::{HashMap, HashSet};

pub const MAX_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_DEPTH: usize = 128;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub strong: bool,
    pub emphasis: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text {
        text: String,
        style: Style,
    },
    Break,
    Image {
        destination: String,
        alt: String,
        link: Option<String>,
    },
}

#[derive(Debug)]
pub enum Content {
    Prose(Vec<Inline>),
    Heading {
        level: u8,
        anchor: String,
        spans: Vec<Inline>,
    },
    Code {
        language: String,
        text: String,
    },
    Rule,
    Table {
        alignments: Vec<Alignment>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
}

#[derive(Debug)]
pub struct Block {
    pub indent: usize,
    pub quotes: usize,
    pub marker: Option<String>,
    pub task: Option<bool>,
    pub content: Content,
}

#[derive(Debug, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub images: Vec<String>,
}

struct TableDraft {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Vec<Inline>>>,
}

struct Builder {
    document: Document,
    spans: Vec<Inline>,
    style: Style,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    items: Vec<Option<String>>,
    quotes: usize,
    task: Option<bool>,
    heading: Option<u8>,
    code: Option<(String, String)>,
    table: Option<TableDraft>,
    image: Option<(String, String, Option<String>, usize)>,
    anchors: HashMap<String, usize>,
    used_anchors: HashSet<String>,
    budget: usize,
}
impl Builder {
    fn charge(&mut self, bytes: usize) -> Result<(), String> {
        // Reserve slack for Vec/HashMap capacity and duplicated anchor/image keys.
        self.budget = self.budget.saturating_add(bytes.saturating_mul(2));
        if self.budget > MAX_BYTES {
            return Err("Markdown model exceeds the 128 MiB preview limit.".into());
        }
        Ok(())
    }
    fn block(&mut self, content: Content) -> Result<(), String> {
        self.charge(std::mem::size_of::<Block>())?;
        let marker = self.items.last_mut().and_then(Option::take);
        self.document.blocks.push(Block {
            indent: self.lists.len(),
            quotes: self.quotes,
            marker,
            task: self.task.take(),
            content,
        });
        Ok(())
    }
    fn flush(&mut self) -> Result<(), String> {
        if self.spans.is_empty() && self.heading.is_none() {
            return Ok(());
        }
        let spans = std::mem::take(&mut self.spans);
        let content = if let Some(level) = self.heading.take() {
            let text = plain_text(&spans);
            let base = heading_anchor(&text);
            let count = self.anchors.entry(base.clone()).or_default();
            let anchor = loop {
                let candidate = if *count == 0 {
                    base.clone()
                } else {
                    format!("{base}-{count}")
                };
                *count += 1;
                if self.used_anchors.insert(candidate.clone()) {
                    break candidate;
                }
            };
            self.charge(anchor.len())?;
            Content::Heading {
                level,
                anchor,
                spans,
            }
        } else {
            Content::Prose(spans)
        };
        self.block(content)
    }
    fn flush_item(&mut self) -> Result<(), String> {
        self.flush()?;
        if self.items.last().is_some_and(Option::is_some) {
            self.block(Content::Prose(Vec::new()))?;
        }
        Ok(())
    }
    fn text(&mut self, text: &str, code: bool) -> Result<(), String> {
        self.charge(text.len().saturating_add(std::mem::size_of::<Inline>()))?;
        if let Some((_, alt, _, _)) = &mut self.image {
            alt.push_str(text);
            return Ok(());
        }
        if let Some((_, content)) = &mut self.code {
            content.push_str(text);
            return Ok(());
        }
        let mut style = self.style.clone();
        style.code |= code;
        self.charge(style.link.as_ref().map_or(0, String::len))?;
        if let Some(Inline::Text {
            text: last,
            style: previous,
        }) = self.spans.last_mut()
            && *previous == style
        {
            last.push_str(text);
        } else {
            self.spans.push(Inline::Text {
                text: text.to_owned(),
                style,
            });
        }
        Ok(())
    }
}

pub fn parse(bytes: &[u8], cancelled: impl Fn() -> bool) -> Result<Document, String> {
    if bytes.len() > MAX_BYTES {
        return Err("Markdown source exceeds the 128 MiB preview limit.".into());
    }
    let source = std::str::from_utf8(bytes).map_err(|_| "Markdown preview requires UTF-8 text.")?;
    let mut builder = Builder {
        document: Document::default(),
        spans: Vec::new(),
        style: Style::default(),
        styles: Vec::new(),
        lists: Vec::new(),
        items: Vec::new(),
        quotes: 0,
        task: None,
        heading: None,
        code: None,
        table: None,
        image: None,
        anchors: HashMap::new(),
        used_anchors: HashSet::new(),
        budget: 0,
    };
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    let mut depth = 0;
    for event in Parser::new_ext(source, options) {
        let is_end = matches!(event, Event::End(_));
        if cancelled() {
            return Err("Preview superseded by a newer document revision.".into());
        }
        if matches!(event, Event::Start(_)) {
            depth += 1;
            if depth > MAX_DEPTH {
                return Err("Markdown nesting exceeds the preview limit of 128.".into());
            }
        }
        if builder.image.is_some() {
            match &event {
                Event::Start(_) => builder.image.as_mut().unwrap().3 += 1,
                Event::End(_) if builder.image.as_ref().unwrap().3 > 0 => {
                    builder.image.as_mut().unwrap().3 -= 1
                }
                Event::End(TagEnd::Image) => {
                    let (destination, alt, link, _) = builder.image.take().unwrap();
                    builder
                        .charge(destination.len() + alt.len() + std::mem::size_of::<Inline>())?;
                    builder.spans.push(Inline::Image {
                        destination,
                        alt,
                        link,
                    });
                }
                Event::Text(text)
                | Event::Code(text)
                | Event::Html(text)
                | Event::InlineHtml(text) => builder.text(text, false)?,
                Event::SoftBreak | Event::HardBreak => builder.text(" ", false)?,
                _ => {}
            }
        } else {
            match event {
                Event::Start(tag) => match tag {
                    Tag::Paragraph | Tag::HtmlBlock => builder.flush()?,
                    Tag::Heading { level, .. } => {
                        builder.flush()?;
                        builder.heading = Some(level as u8);
                    }
                    Tag::BlockQuote(_) => {
                        builder.flush()?;
                        builder.quotes += 1;
                    }
                    Tag::List(start) => {
                        builder.flush_item()?;
                        builder.lists.push(start);
                    }
                    Tag::Item => {
                        builder.flush()?;
                        let marker = if let Some(Some(next)) = builder.lists.last_mut() {
                            let value = format!("{next}.");
                            *next = next.saturating_add(1);
                            value
                        } else {
                            "•".into()
                        };
                        builder.items.push(Some(marker));
                    }
                    Tag::CodeBlock(kind) => {
                        builder.flush()?;
                        let language = match kind {
                            CodeBlockKind::Fenced(value) => value.into_string(),
                            CodeBlockKind::Indented => String::new(),
                        };
                        builder.charge(language.len())?;
                        builder.code = Some((language, String::new()));
                    }
                    Tag::Table(alignments) => {
                        builder.flush()?;
                        builder.table = Some(TableDraft {
                            alignments,
                            rows: Vec::new(),
                        });
                    }
                    Tag::TableHead | Tag::TableRow => {
                        builder.charge(std::mem::size_of::<Vec<Vec<Inline>>>())?;
                        if let Some(table) = &mut builder.table {
                            table.rows.push(Vec::new());
                        }
                    }
                    Tag::TableCell => builder.spans.clear(),
                    Tag::Image { dest_url, .. } => {
                        builder.image = Some((
                            dest_url.into_string(),
                            String::new(),
                            builder.style.link.clone(),
                            0,
                        ));
                    }
                    Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } => {
                        builder.styles.push(builder.style.clone());
                        match tag {
                            Tag::Emphasis => builder.style.emphasis = true,
                            Tag::Strong => builder.style.strong = true,
                            Tag::Strikethrough => builder.style.strike = true,
                            Tag::Link { dest_url, .. } => {
                                builder.charge(dest_url.len())?;
                                builder.style.link = Some(dest_url.into_string());
                            }
                            _ => unreachable!(),
                        }
                    }
                    _ => {}
                },
                Event::End(tag) => match tag {
                    TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::HtmlBlock => {
                        builder.flush()?
                    }
                    TagEnd::BlockQuote(_) => {
                        builder.flush()?;
                        builder.quotes = builder.quotes.saturating_sub(1);
                    }
                    TagEnd::Item => {
                        builder.flush_item()?;
                        builder.items.pop();
                    }
                    TagEnd::List(_) => {
                        builder.flush()?;
                        builder.lists.pop();
                    }
                    TagEnd::CodeBlock => {
                        if let Some((language, text)) = builder.code.take() {
                            builder.block(Content::Code { language, text })?;
                        }
                    }
                    TagEnd::TableCell => {
                        builder.charge(std::mem::size_of::<Vec<Inline>>())?;
                        if let Some(table) = &mut builder.table
                            && let Some(row) = table.rows.last_mut()
                        {
                            row.push(std::mem::take(&mut builder.spans));
                        }
                    }
                    TagEnd::Table => {
                        if let Some(TableDraft { alignments, rows }) = builder.table.take() {
                            builder.block(Content::Table { alignments, rows })?;
                        }
                    }
                    TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                        builder.style = builder.styles.pop().unwrap_or_default()
                    }
                    _ => {}
                },
                Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                    builder.text(&text, false)?
                }
                Event::Code(text) => builder.text(&text, true)?,
                Event::SoftBreak => builder.text(" ", false)?,
                Event::HardBreak => {
                    builder.charge(std::mem::size_of::<Inline>())?;
                    builder.spans.push(Inline::Break);
                }
                Event::Rule => {
                    builder.flush()?;
                    builder.block(Content::Rule)?;
                }
                Event::TaskListMarker(checked) => builder.task = Some(checked),
                Event::FootnoteReference(value)
                | Event::InlineMath(value)
                | Event::DisplayMath(value) => builder.text(&value, false)?,
            }
        }
        // Starts and ends remain balanced even while consuming image alt text.
        if is_end {
            depth = depth.saturating_sub(1);
        }
    }
    builder.flush()?;
    let mut seen = HashSet::new();
    for block in &builder.document.blocks {
        match &block.content {
            Content::Prose(spans) | Content::Heading { spans, .. } => {
                collect_images(spans, &mut seen, &mut builder.document.images)
            }
            Content::Table { rows, .. } => {
                for row in rows {
                    for spans in row {
                        collect_images(spans, &mut seen, &mut builder.document.images);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(builder.document)
}

fn collect_images(spans: &[Inline], seen: &mut HashSet<String>, output: &mut Vec<String>) {
    for span in spans {
        if let Inline::Image { destination, .. } = span
            && seen.insert(destination.clone())
        {
            output.push(destination.clone());
        }
    }
}

pub fn plain_text(spans: &[Inline]) -> String {
    spans
        .iter()
        .map(|span| match span {
            Inline::Text { text, .. } => text.as_str(),
            Inline::Break => " ",
            Inline::Image { alt, .. } => alt.as_str(),
        })
        .collect()
}

pub fn heading_anchor(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '_' | '-') {
                Some(ch)
            } else if ch.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn doc(source: &str) -> Document {
        parse(source.as_bytes(), || false).unwrap()
    }
    #[test]
    fn nested_blocks_styles_links_images_and_literal_html() {
        let document = doc(
            "# Hello *world*\n\n> - **bold** and ~~gone~~ [link](a.md)\n>   - [x] child ![*picture*](img.png)\n\n<div>literal</div>\n\n```rust\nfn main() {}\n```\n",
        );
        assert!(
            matches!(&document.blocks[0].content, Content::Heading { level: 1, anchor, .. } if anchor == "hello-world")
        );
        assert_eq!(document.blocks[1].quotes, 1);
        assert_eq!(document.blocks[1].marker.as_deref(), Some("•"));
        assert_eq!(document.blocks[2].indent, 2);
        assert_eq!(document.blocks[2].task, Some(true));
        assert_eq!(document.images, ["img.png"]);
        assert!(
            matches!(&document.blocks[3].content, Content::Prose(spans) if plain_text(spans).contains("<div>literal</div>"))
        );
        assert!(
            matches!(&document.blocks[4].content, Content::Code { language, text } if language == "rust" && text == "fn main() {}\n")
        );
        let Content::Prose(spans) = &document.blocks[1].content else {
            panic!()
        };
        assert!(
            spans
                .iter()
                .any(|span| matches!(span, Inline::Text { style, .. } if style.strong))
        );
        assert!(
            spans
                .iter()
                .any(|span| matches!(span, Inline::Text { style, .. } if style.strike))
        );
        assert!(spans.iter().any(|span| matches!(span, Inline::Text { style, .. } if style.link.as_deref() == Some("a.md"))));
    }
    #[test]
    fn tables_lists_breaks_and_duplicate_anchors() {
        let document = doc(
            "## Repeat\n\n## Repeat\n\n3. first  \n   next\n4. second\n\n| left | right |\n| :--- | ---: |\n| **x** | `y` |\n",
        );
        assert!(
            matches!(&document.blocks[1].content, Content::Heading { anchor, .. } if anchor == "repeat-1")
        );
        assert_eq!(document.blocks[2].marker.as_deref(), Some("3."));
        assert_eq!(document.blocks[3].marker.as_deref(), Some("4."));
        let Content::Table { alignments, rows } = &document.blocks[4].content else {
            panic!()
        };
        assert_eq!(alignments, &[Alignment::Left, Alignment::Right]);
        assert_eq!(rows.len(), 2);
        assert_eq!(plain_text(&rows[1][1]), "y");
    }
    #[test]
    fn source_limits_invalid_utf8_nesting_and_cancellation() {
        assert!(parse(&[255], || false).is_err());
        assert!(parse(b"hello", || true).is_err());
        assert!(
            parse(
                format!("{}deep", "> ".repeat(MAX_DEPTH + 1)).as_bytes(),
                || false
            )
            .unwrap_err()
            .contains("nesting")
        );
    }
    #[test]
    fn empty_list_items_remain_visible_and_heading_ids_do_not_collide() {
        let document = doc("-\n- next\n\n# Name\n# Name-1\n# Name\n");
        assert_eq!(document.blocks[0].marker.as_deref(), Some("•"));
        assert_eq!(document.blocks[0].indent, 1);
        assert!(matches!(&document.blocks[0].content,Content::Prose(spans) if spans.is_empty()));
        let anchors: Vec<_> = document
            .blocks
            .iter()
            .filter_map(|block| match &block.content {
                Content::Heading { anchor, .. } => Some(anchor.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(anchors, ["name", "name-1", "name-2"]);
    }
}
