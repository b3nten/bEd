//! Markdown as blocks to draw: headings, paragraphs, lists, quotes (GitHub's
//! `[!NOTE]` callouts too), tables, code and rules, with inline runs. Code
//! blocks remember where their text is in the source, so an analysis of the
//! source (which colours fenced code by its language) colours them.
use crate::theme::SyntaxRole;
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use std::{collections::HashMap, ops::Range};

#[derive(Debug, Default, PartialEq)]
pub struct Markdown {
    pub blocks: Vec<Block>,
}

#[derive(Debug, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        text: Vec<Run>,
        /// GitHub's anchor (`#getting-started`), unique in the document.
        anchor: String,
    },
    Paragraph(Vec<Run>),
    Code(Code),
    Quote {
        kind: Option<Callout>,
        blocks: Vec<Block>,
    },
    List {
        /// The first number of an ordered list.
        start: Option<u64>,
        items: Vec<Item>,
    },
    Table {
        alignments: Vec<Alignment>,
        head: Vec<Vec<Run>>,
        rows: Vec<Vec<Vec<Run>>>,
    },
    Rule,
    Html(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Callout {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Debug, Default, PartialEq)]
pub struct Item {
    /// A task list item's box.
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Code {
    pub language: String,
    pub text: String,
    /// Where pieces of `text` (from `at`) are in the source.
    pieces: Vec<(usize, Range<usize>)>,
    /// Roles over `text`, once coloured: sorted, disjoint.
    pub highlights: Vec<(Range<usize>, SyntaxRole)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub style: Style,
    pub link: Option<String>,
    /// An image's source; `text` is its description.
    pub image: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    pub strong: bool,
    pub emphasis: bool,
    pub strike: bool,
    pub code: bool,
    pub image: bool,
}

/// An open container, while its events arrive.
enum Open {
    Quote(Option<Callout>, Vec<Block>),
    List(Option<u64>, Vec<Item>),
    Item(Item),
    Table {
        alignments: Vec<Alignment>,
        head: Vec<Vec<Run>>,
        rows: Vec<Vec<Vec<Run>>>,
        in_head: bool,
    },
}

#[derive(Default)]
struct Builder {
    root: Vec<Block>,
    open: Vec<Open>,
    runs: Vec<Run>,
    style: Style,
    link: Option<String>,
    image: Option<String>,
    code: Option<Code>,
    html: Option<String>,
    heading: Option<u8>,
    anchors: HashMap<String, usize>,
}

impl Builder {
    fn blocks(&mut self) -> &mut Vec<Block> {
        for open in self.open.iter_mut().rev() {
            match open {
                Open::Quote(_, blocks) | Open::Item(Item { blocks, .. }) => return blocks,
                // Lists and tables hold no blocks of their own.
                Open::List(..) | Open::Table { .. } => {}
            }
        }
        &mut self.root
    }

    fn push(&mut self, block: Block) {
        self.flush();
        self.blocks().push(block);
    }

    /// Text outside a paragraph (a tight list item's) becomes one.
    fn flush(&mut self) {
        if !self.runs.is_empty() {
            let runs = std::mem::take(&mut self.runs);
            self.blocks().push(Block::Paragraph(runs));
        }
    }

    fn run(&mut self, text: &str, style: Style) {
        // Neighbours alike in style and link join, so wrapping sees words.
        if let Some(last) = self.runs.last_mut()
            && last.style == style
            && last.link == self.link
            && last.image == self.image
        {
            last.text.push_str(text);
            return;
        }
        self.runs.push(Run {
            text: text.into(),
            style,
            link: self.link.clone(),
            image: self.image.clone(),
        });
    }

    fn anchor(&mut self, runs: &[Run]) -> String {
        let text: String = runs.iter().map(|run| run.text.as_str()).collect();
        let base = slug(&text);
        let count = self.anchors.entry(base.clone()).or_insert(0);
        let anchor = match *count {
            0 => base,
            n => format!("{base}-{n}"),
        };
        *count += 1;
        anchor
    }

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        if let Some(code) = &mut self.code {
            match event {
                Event::Text(text) => {
                    code.pieces.push((code.text.len(), range));
                    code.text.push_str(&text);
                }
                Event::End(TagEnd::CodeBlock) => {
                    let mut code = self.code.take().unwrap();
                    let trimmed = code.text.trim_end_matches('\n').len();
                    code.text.truncate(trimmed);
                    self.push(Block::Code(code));
                }
                _ => {}
            }
            return;
        }
        if let Some(html) = &mut self.html {
            match event {
                Event::Html(text) | Event::Text(text) => html.push_str(&text),
                Event::End(TagEnd::HtmlBlock) => {
                    let html = self.html.take().unwrap();
                    let html = html.trim_end();
                    // Comments are for whoever edits the file.
                    if !(html.starts_with("<!--") && html.ends_with("-->")) {
                        self.push(Block::Html(html.into()));
                    }
                }
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.run(&text, self.style),
            Event::Code(text) | Event::InlineMath(text) => self.run(
                &text,
                Style {
                    code: true,
                    ..self.style
                },
            ),
            Event::DisplayMath(text) => self.push(Block::Code(Code {
                language: "math".into(),
                text: text.trim().into(),
                ..Default::default()
            })),
            Event::InlineHtml(html) => {
                if html.trim_start_matches('<').trim_start().starts_with("br") {
                    self.run("\n", self.style);
                }
            }
            Event::Html(text) => self.push(Block::Html(text.trim_end().into())),
            Event::FootnoteReference(name) => self.run(&format!("[{name}]"), self.style),
            Event::SoftBreak => self.run(" ", self.style),
            Event::HardBreak => self.run("\n", self.style),
            Event::Rule => self.push(Block::Rule),
            Event::TaskListMarker(checked) => {
                if let Some(Open::Item(item)) = self.open.last_mut() {
                    item.checked = Some(checked);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.flush(),
            Tag::Heading { level, .. } => {
                self.flush();
                self.heading = Some(match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                });
            }
            Tag::BlockQuote(kind) => {
                self.flush();
                let kind = kind.map(|kind| match kind {
                    BlockQuoteKind::Note => Callout::Note,
                    BlockQuoteKind::Tip => Callout::Tip,
                    BlockQuoteKind::Important => Callout::Important,
                    BlockQuoteKind::Warning => Callout::Warning,
                    BlockQuoteKind::Caution => Callout::Caution,
                });
                self.open.push(Open::Quote(kind, Vec::new()));
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split([' ', ',', '{']).next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(Code {
                    language,
                    ..Default::default()
                });
            }
            Tag::HtmlBlock => {
                self.flush();
                self.html = Some(String::new());
            }
            Tag::List(start) => {
                self.flush();
                self.open.push(Open::List(start, Vec::new()));
            }
            Tag::Item => self.open.push(Open::Item(Item::default())),
            Tag::Table(alignments) => {
                self.flush();
                self.open.push(Open::Table {
                    alignments,
                    head: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(Open::Table { in_head, .. }) = self.open.last_mut() {
                    *in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(Open::Table { rows, .. }) = self.open.last_mut() {
                    rows.push(Vec::new());
                }
            }
            Tag::TableCell => self.runs.clear(),
            Tag::Emphasis => self.style.emphasis = true,
            Tag::Strong => self.style.strong = true,
            Tag::Strikethrough => self.style.strike = true,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.into_string()),
            Tag::Image { dest_url, .. } => {
                self.style.image = true;
                self.image = Some(dest_url.into_string());
                // An image with no description still shows.
                self.run("", self.style);
            }
            Tag::FootnoteDefinition(name) => {
                self.flush();
                self.run(&format!("[{name}] "), self.style);
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::FootnoteDefinition => self.flush(),
            TagEnd::Heading(_) => {
                let text = std::mem::take(&mut self.runs);
                let anchor = self.anchor(&text);
                let level = self.heading.take().unwrap_or(1);
                self.push(Block::Heading {
                    level,
                    text,
                    anchor,
                });
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                if let Some(Open::Quote(kind, blocks)) = self.open.pop() {
                    self.push(Block::Quote { kind, blocks });
                }
            }
            TagEnd::Item => {
                self.flush();
                let item = match self.open.pop() {
                    Some(Open::Item(item)) => item,
                    _ => return,
                };
                if let Some(Open::List(_, items)) = self.open.last_mut() {
                    items.push(item);
                }
            }
            TagEnd::List(_) => {
                if let Some(Open::List(start, items)) = self.open.pop() {
                    self.push(Block::List { start, items });
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.runs);
                if let Some(Open::Table {
                    head,
                    rows,
                    in_head,
                    ..
                }) = self.open.last_mut()
                {
                    if *in_head {
                        head.push(cell);
                    } else if let Some(row) = rows.last_mut() {
                        row.push(cell);
                    }
                }
            }
            TagEnd::TableHead => {
                if let Some(Open::Table { in_head, .. }) = self.open.last_mut() {
                    *in_head = false;
                }
            }
            TagEnd::Table => {
                if let Some(Open::Table {
                    alignments,
                    head,
                    rows,
                    ..
                }) = self.open.pop()
                {
                    self.push(Block::Table {
                        alignments,
                        head,
                        rows,
                    });
                }
            }
            TagEnd::Emphasis => self.style.emphasis = false,
            TagEnd::Strong => self.style.strong = false,
            TagEnd::Strikethrough => self.style.strike = false,
            TagEnd::Link => self.link = None,
            TagEnd::Image => {
                self.style.image = false;
                self.image = None;
            }
            _ => {}
        }
    }
}

/// GitHub's heading anchors: lowercase, spaces to hyphens, most punctuation
/// gone.
pub fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

pub fn parse(text: &str) -> Markdown {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_MATH;
    let mut builder = Builder::default();
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        builder.event(event, range);
    }
    // An unclosed container (there shouldn't be one) keeps its text.
    builder.flush();
    Markdown {
        blocks: builder.root,
    }
}

impl Markdown {
    /// Colour code blocks from `spans` over the source they came from
    /// (sorted, disjoint byte ranges, as an analysis of the source gives).
    pub fn colour(&mut self, spans: &[(Range<usize>, SyntaxRole)]) {
        self.visit_code(&mut |code| code.colour(spans));
    }

    /// Each code block, in order.
    pub fn visit_code(&mut self, visit: &mut impl FnMut(&mut Code)) {
        fn walk(blocks: &mut [Block], visit: &mut impl FnMut(&mut Code)) {
            for block in blocks {
                match block {
                    Block::Code(code) => visit(code),
                    Block::Quote { blocks, .. } => walk(blocks, visit),
                    Block::List { items, .. } => {
                        for item in items {
                            walk(&mut item.blocks, visit);
                        }
                    }
                    _ => {}
                }
            }
        }
        walk(&mut self.blocks, visit);
    }
}

impl Code {
    /// Colour it from `spans` over the source it came from.
    pub fn colour(&mut self, spans: &[(Range<usize>, SyntaxRole)]) {
        self.highlights.clear();
        for (i, (at, source)) in self.pieces.iter().enumerate() {
            let end = self
                .pieces
                .get(i + 1)
                .map_or(self.text.len(), |(next, _)| *next)
                .min(self.text.len());
            // A piece the parser rewrote (tabs, say) can't be mapped.
            if end.saturating_sub(*at) > source.len() || *at >= end {
                continue;
            }
            let first = spans.partition_point(|(range, _)| range.end <= source.start);
            for (range, role) in spans[first..]
                .iter()
                .take_while(|(range, _)| range.start < source.end)
            {
                let start = range.start.max(source.start) - source.start + at;
                let stop = (range.end.min(source.end) - source.start + at).min(end);
                if start < stop
                    && self.text.is_char_boundary(start)
                    && self.text.is_char_boundary(stop)
                {
                    self.highlights.push((start..stop, *role));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(runs: &[Run]) -> String {
        runs.iter().map(|run| run.text.as_str()).collect()
    }

    #[test]
    fn blocks_nest_as_written() {
        let markdown = parse(
            "# Title\n\nSome *very* **bold** `code` and [a link](https://x.org).\n\n\
             - one\n- [x] done\n  - nested\n\n1. first\n2. second\n\n\
             > [!NOTE]\n> Careful.\n\n---\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n",
        );
        let blocks = &markdown.blocks;
        assert!(matches!(&blocks[0], Block::Heading { level: 1, anchor, .. } if anchor == "title"));
        let Block::Paragraph(runs) = &blocks[1] else {
            panic!("{:?}", blocks[1])
        };
        assert_eq!(text(runs), "Some very bold code and a link.");
        assert!(runs.iter().any(|r| r.style.emphasis && r.text == "very"));
        assert!(runs.iter().any(|r| r.style.code && r.text == "code"));
        assert!(
            runs.iter()
                .any(|r| r.link.as_deref() == Some("https://x.org"))
        );
        let Block::List { start: None, items } = &blocks[2] else {
            panic!("{:?}", blocks[2])
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].checked, Some(true));
        assert!(matches!(&items[1].blocks[0], Block::Paragraph(r) if text(r) == "done"));
        assert!(matches!(&items[1].blocks[1], Block::List { .. }));
        assert!(matches!(&blocks[3], Block::List { start: Some(1), items } if items.len() == 2));
        assert!(
            matches!(&blocks[4], Block::Quote { kind: Some(Callout::Note), blocks } if blocks.len() == 1)
        );
        assert_eq!(blocks[5], Block::Rule);
        let Block::Table {
            alignments,
            head,
            rows,
        } = &blocks[6]
        else {
            panic!("{:?}", blocks[6])
        };
        assert_eq!(alignments, &[Alignment::None, Alignment::Right]);
        assert_eq!(text(&head[1]), "b");
        assert_eq!(text(&rows[0][1]), "2");
    }

    #[test]
    fn images_keep_their_source_and_link() {
        let markdown =
            parse("See ![a *logo*](img/logo.png) and [![](badge.svg)](https://ci.org).\n");
        let Block::Paragraph(runs) = &markdown.blocks[0] else {
            panic!("{:?}", markdown.blocks[0])
        };
        let images: Vec<_> = runs.iter().filter(|run| run.image.is_some()).collect();
        assert_eq!(images.len(), 3, "{runs:?}");
        assert_eq!(images[0].image.as_deref(), Some("img/logo.png"));
        assert_eq!(images[0].text, "a ");
        assert!(images[1].style.emphasis && images[1].text == "logo");
        assert_eq!(images[2].image.as_deref(), Some("badge.svg"));
        assert_eq!(images[2].text, "");
        assert_eq!(images[2].link.as_deref(), Some("https://ci.org"));
    }

    #[test]
    fn code_knows_where_it_came_from() {
        let source = "Intro\n\n> ```rust\n> fn main() {}\n> let x = 1;\n> ```\n";
        let markdown = parse(source);
        let Block::Quote { blocks, .. } = &markdown.blocks[1] else {
            panic!()
        };
        let Block::Code(code) = &blocks[0] else {
            panic!()
        };
        assert_eq!(code.language, "rust");
        assert_eq!(code.text, "fn main() {}\nlet x = 1;");
        for (at, range) in &code.pieces {
            let piece = &source[range.clone()];
            assert!(
                code.text[*at..].starts_with(piece.trim_end_matches('\n')),
                "{piece:?}"
            );
        }
    }

    #[test]
    fn anchors_are_githubs_and_unique() {
        let markdown = parse("## Getting Started!\n\n## Getting started\n");
        let anchors: Vec<_> = markdown
            .blocks
            .iter()
            .map(|b| match b {
                Block::Heading { anchor, .. } => anchor.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(anchors, ["getting-started", "getting-started-1"]);
    }

    #[test]
    fn comments_are_hidden_and_html_kept() {
        let markdown = parse("<!-- note -->\n\n<div>hi</div>\n");
        assert_eq!(markdown.blocks, [Block::Html("<div>hi</div>".into())]);
    }
}
