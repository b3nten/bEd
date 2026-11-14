//! Documentation from a language server (hover, completion entries), drawn
//! by `bedtime_ui::markdown::MarkdownView` in its compact style: its code is
//! coloured as the file's language (or its fence's) on a thread of its own,
//! never while painting. Web links open in the browser.
use super::Markup;
use bedtime_ui::{
    UiContext, egui,
    markdown::{MarkdownStyle, MarkdownView},
};

pub(super) struct Documentation {
    text: String,
    markdown: bool,
    language: String,
    view: MarkdownView,
}

/// Make `current` show `markup`, unless it already does.
pub(super) fn prepare(current: &mut Option<Documentation>, markup: &Markup, language: &str) {
    if current.as_ref().is_some_and(|d| {
        d.text == markup.text && d.markdown == markup.markdown && d.language == language
    }) {
        return;
    }
    let view = if markup.markdown {
        MarkdownView::new(&markup.text).code_language(language)
    } else {
        MarkdownView::plain(&markup.text)
    };
    *current = Some(Documentation {
        text: markup.text.clone(),
        markdown: markup.markdown,
        language: language.into(),
        view: view.style(MarkdownStyle::Compact),
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
    pub(super) fn show(&mut self, ui: &mut egui::Ui, context: &UiContext<'_>) {
        if let Some(link) = self.view.show(ui, context)
            && link.contains("://")
        {
            // Drawing has no world: it's opened once the frame is drawn.
            bedtime_ui::request_open_externally(ui.ctx(), link);
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
        use bedtime_ui::markdown::Block;
        let mut current = None;
        let markup = Markup {
            text: "```rust\npub fn new() -> Vec<u8>\n```\n\n---\n\nCon**cat**enate `变量` and [résumé](https://example.org).".into(),
            markdown: true,
        };
        prepare(&mut current, &markup, "rust");
        let blocks = &current.as_ref().unwrap().view.markdown().blocks;
        let Block::Code(code) = &blocks[0] else {
            panic!("signature not code")
        };
        assert_eq!(code.text, "pub fn new() -> Vec<u8>");
        assert!(matches!(blocks[1], Block::Rule));
        let Block::Paragraph(runs) = &blocks[2] else {
            panic!("documentation not prose")
        };
        assert_eq!(
            runs.iter().map(|r| r.text.as_str()).collect::<String>(),
            "Concatenate 变量 and résumé."
        );
        assert!(runs.iter().any(|r| r.style.strong));
        assert!(runs.iter().any(|r| r.style.code));
        assert!(
            runs.iter()
                .any(|r| r.link.as_deref() == Some("https://example.org"))
        );
    }

    #[test]
    fn plain_documentation_is_not_read_as_markdown() {
        let mut current = None;
        let markup = Markup {
            text: "# not a heading".into(),
            markdown: false,
        };
        prepare(&mut current, &markup, "rust");
        assert!(matches!(
            current.unwrap().view.markdown().blocks.as_slice(),
            [bedtime_ui::markdown::Block::Paragraph(_)]
        ));
    }
}
