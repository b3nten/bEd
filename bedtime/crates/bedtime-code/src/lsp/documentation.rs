//! Documentation from a language server (hover, completion entries): wrapped
//! prose and syntax-coloured code. Markdown is parsed once per text; its code
//! is coloured by a syntax analysis on a worker, never while painting.
use super::Markup;
use crate::{
    editor::syntax::SyntaxPalette,
    syntax::{AnalysisSession, AnalysisStatus},
};
use bedtime_ui::egui;
use bedtime_workbench::{ManagedTask, ManagedWork};
use bevy::prelude::*;
use pulldown_cmark::{CodeBlockKind, Event, Tag, TagEnd};
use std::{ops::Range, sync::Arc, time::Duration};

#[derive(Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
}

struct Run {
    text: String,
    style: Style,
}

enum Block {
    Prose(Vec<Run>),
    Code { text: String, source: Range<usize> },
    Rule,
}

pub(super) struct Documentation {
    text: String,
    markdown: bool,
    language: String,
    blocks: Vec<Block>,
    analysis: Option<(AnalysisSession, ManagedTask<()>)>,
}

fn blocks(markup: &Markup, language: &str) -> (Vec<Block>, String) {
    if !markup.markdown {
        return (
            vec![Block::Prose(vec![Run {
                text: markup.text.clone(),
                style: Style::default(),
            }])],
            String::new(),
        );
    }
    let mut blocks = Vec::new();
    let mut runs = Vec::new();
    let mut style = Style::default();
    let mut code: Option<(String, String)> = None;
    let mut source = String::new();
    let flush = |blocks: &mut Vec<Block>, runs: &mut Vec<Run>| {
        if !runs.is_empty() {
            blocks.push(Block::Prose(std::mem::take(runs)));
        }
    };
    for event in pulldown_cmark::Parser::new(&markup.text) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut blocks, &mut runs);
                let name = match kind {
                    CodeBlockKind::Fenced(name) => name
                        .split_whitespace()
                        .next()
                        .unwrap_or(language)
                        .to_owned(),
                    CodeBlockKind::Indented => language.into(),
                };
                code = Some((
                    if name.is_empty() {
                        language.into()
                    } else {
                        name
                    },
                    String::new(),
                ));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((language, text)) = code.take() {
                    // Choose a fence that cannot terminate inside a code example.
                    let fence = "`".repeat(
                        text.lines()
                            .map(|l| l.chars().take_while(|c| *c == '`').count() + 1)
                            .max()
                            .unwrap_or(3)
                            .max(3),
                    );
                    source.push_str(&format!("{fence}{language}\n"));
                    let start = source.len();
                    source.push_str(&text);
                    let end = source.len();
                    source.push_str(&format!("\n{fence}\n\n"));
                    blocks.push(Block::Code {
                        text: text.trim_end_matches('\n').to_owned(),
                        source: start..end,
                    });
                }
            }
            Event::Text(text) => {
                if let Some((_, body)) = &mut code {
                    body.push_str(&text);
                } else {
                    runs.push(Run {
                        text: text.into_string(),
                        style,
                    });
                }
            }
            Event::Code(text) => runs.push(Run {
                text: text.into_string(),
                style: Style {
                    code: true,
                    ..style
                },
            }),
            Event::Start(Tag::Strong) => style.bold = true,
            Event::End(TagEnd::Strong) => style.bold = false,
            Event::Start(Tag::Emphasis) => style.italic = true,
            Event::End(TagEnd::Emphasis) => style.italic = false,
            Event::Start(Tag::Heading { .. }) => style.bold = true,
            Event::End(TagEnd::Heading(_)) => {
                style.bold = false;
                flush(&mut blocks, &mut runs);
            }
            Event::Start(Tag::Item) => {
                flush(&mut blocks, &mut runs);
                runs.push(Run {
                    text: "• ".into(),
                    style,
                });
            }
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Item) => {
                flush(&mut blocks, &mut runs)
            }
            Event::SoftBreak => runs.push(Run {
                text: " ".into(),
                style,
            }),
            Event::HardBreak => runs.push(Run {
                text: "\n".into(),
                style,
            }),
            Event::Rule => {
                flush(&mut blocks, &mut runs);
                blocks.push(Block::Rule);
            }
            _ => {}
        }
    }
    flush(&mut blocks, &mut runs);
    (blocks, source)
}

pub(super) fn prepare(
    world: &mut World,
    panel: Entity,
    current: &mut Option<Documentation>,
    markup: &Markup,
    language: &str,
) {
    if current.as_ref().is_some_and(|d| {
        d.text == markup.text && d.markdown == markup.markdown && d.language == language
    }) {
        return;
    }
    let (blocks, source) = blocks(markup, language);
    let mut analysis = None;
    if !source.is_empty() {
        let (mut session, worker) = AnalysisSession::with_worker();
        if let Ok(task) = world.resource_mut::<ManagedWork>().spawn_task(
            Some(panel),
            "Colouring documentation",
            move |cancel| worker.run(|| cancel.is_cancelled()),
        ) {
            session.request(Arc::new(source), 0, "markdown", None);
            analysis = Some((session, task));
        }
    }
    *current = Some(Documentation {
        text: markup.text.clone(),
        markdown: markup.markdown,
        language: language.into(),
        blocks,
        analysis,
    });
}

pub(super) fn style(ui: &mut egui::Ui) {
    // Match bEd's compact documentation typography while keeping native text
    // selection and the active theme's syntax palette.
    let size = egui::TextStyle::Body.resolve(ui.style()).size * 0.9;
    for font in ui.style_mut().text_styles.values_mut() {
        font.size = size;
    }
    ui.spacing_mut().item_spacing = egui::vec2(size * 0.35, size * 0.25);
}

pub(super) fn scroll_area(ui: &egui::Ui, max_height: f32) -> egui::ScrollArea {
    // Areas remember their previous content size. Without this floor a short
    // hover can leave later documentation trapped in the default 64-point viewport.
    egui::ScrollArea::vertical()
        .min_scrolled_height((6.0 * ui.text_style_height(&egui::TextStyle::Body)).min(max_height))
        .max_height(max_height)
}

impl Documentation {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, palette: &SyntaxPalette) {
        if let Some((session, _)) = &mut self.analysis {
            session.poll();
            if session.status() == &AnalysisStatus::Pending {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
        }
        let observation = self.analysis.as_ref().and_then(|(s, _)| s.observation());
        let text_color = ui.visuals().text_color();
        let body = egui::TextStyle::Body.resolve(ui.style());
        let mono = egui::TextStyle::Monospace.resolve(ui.style());
        for block in &self.blocks {
            let mut job = egui::text::LayoutJob::default();
            match block {
                Block::Rule => {
                    ui.separator();
                    continue;
                }
                Block::Prose(runs) => {
                    for run in runs {
                        let font = if run.style.bold {
                            bedtime_ui::fonts::monospace_bold(body.size)
                        } else if run.style.code {
                            mono.clone()
                        } else {
                            body.clone()
                        };
                        job.append(
                            &run.text,
                            0.0,
                            egui::TextFormat {
                                font_id: font,
                                color: text_color,
                                italics: run.style.italic,
                                background: if run.style.code {
                                    ui.visuals().code_bg_color
                                } else {
                                    egui::Color32::TRANSPARENT
                                },
                                ..Default::default()
                            },
                        );
                    }
                }
                Block::Code { text, source } => {
                    let spans = observation
                        .map(|o| o.highlights_in(source.clone()))
                        .unwrap_or_default();
                    let mut at = 0;
                    for span in spans {
                        let start = span
                            .range
                            .start
                            .saturating_sub(source.start)
                            .min(text.len());
                        let end = span.range.end.saturating_sub(source.start).min(text.len());
                        if start > at {
                            job.append(
                                &text[at..start],
                                0.0,
                                egui::TextFormat {
                                    font_id: mono.clone(),
                                    color: text_color,
                                    ..Default::default()
                                },
                            );
                        }
                        if end > start {
                            job.append(
                                &text[start..end],
                                0.0,
                                egui::TextFormat {
                                    font_id: mono.clone(),
                                    color: palette.color(&span.capture, text_color),
                                    ..Default::default()
                                },
                            );
                        }
                        at = end;
                    }
                    if at < text.len() {
                        job.append(
                            &text[at..],
                            0.0,
                            egui::TextFormat {
                                font_id: mono.clone(),
                                color: text_color,
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            job.wrap.max_width = ui.available_width();
            ui.add(egui::Label::new(job).wrap().selectable(true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_documentation_grows_to_six_lines_after_a_short_hover() {
        let ctx = egui::Context::default();
        let mut minimum = 0.0;
        let mut height = 0.0;
        for long in [false, false, true, true, true] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::Area::new(egui::Id::new("documentation-height"))
                    .fixed_pos(egui::pos2(20.0, 20.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            style(ui);
                            ui.set_width(360.0);
                            minimum = 6.0 * ui.text_style_height(&egui::TextStyle::Body);
                            height = scroll_area(ui, 320.0)
                                .show(ui, |ui| {
                                    ui.label(if long {
                                        "Documentation line\n".repeat(12)
                                    } else {
                                        "Short hover".into()
                                    });
                                })
                                .inner_rect
                                .height();
                        });
                    });
            });
            output.textures_delta.clear();
        }
        assert!(
            height >= minimum,
            "{height} points cannot fit six lines ({minimum})"
        );
    }

    #[test]
    fn rust_hover_keeps_code_prose_rules_and_inline_words_separate() {
        let (blocks,source)=blocks(&Markup {text:"```rust\npub fn new() -> Vec<u8>\n```\n\n---\n\nCon**cat**enate `变量` and [résumé](https://example.org).".into(),markdown:true},"rust");
        assert!(source.contains("```rust"));
        let Block::Code { text, .. } = &blocks[0] else {
            panic!("signature not code")
        };
        assert_eq!(text, "pub fn new() -> Vec<u8>");
        assert!(matches!(blocks[1], Block::Rule));
        let Block::Prose(runs) = &blocks[2] else {
            panic!("documentation not prose")
        };
        assert_eq!(
            runs.iter().map(|r| r.text.as_str()).collect::<String>(),
            "Concatenate 变量 and résumé."
        );
        assert!(runs.iter().any(|r| r.style.bold));
        assert!(runs.iter().any(|r| r.style.code));
    }
}
