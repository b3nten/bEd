//! A read-only text viewer, until the editor comes back: the file's lines,
//! scrolled to where it was opened.
use bedtime_document::{TextBuffer, TextOpenState, TextPosition};
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::{
    DocumentHandle, DocumentReference, DocumentStatus, Panel, PanelInstance, Viewer,
    WorkbenchWorldExt,
};
use bevy::prelude::*;

#[derive(Component)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct TextViewer {
    /// Where to scroll once the text is loaded.
    pending: Option<TextPosition>,
}

impl Viewer<TextBuffer> for TextViewer {
    fn new(_: DocumentHandle<TextBuffer>) -> Self {
        Self { pending: None }
    }

    fn apply(world: &mut World, panel: Entity, state: &TextOpenState) {
        if let Some(mut viewer) = world.get_mut::<TextViewer>(panel)
            && let Some(cursor) = &state.cursor
        {
            viewer.pending = Some(*cursor);
        }
    }
}

impl Panel for TextViewer {
    const LABEL: &'static str = "Text";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = world.get::<DocumentReference>(panel).map(|r| r.document()) else {
            return;
        };
        let Ok(info) = world.workbench().inspect_document(document) else {
            return;
        };
        if let Some(mut instance) = world.get_mut::<PanelInstance>(panel)
            && instance.title != info.title
        {
            instance.title = info.title;
        }
        match info.status {
            DocumentStatus::Loading => {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            }
            DocumentStatus::Failed(error) => {
                egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                    ui.label(format!("This file can't be shown: {error}"));
                });
                return;
            }
            DocumentStatus::Ready => {}
        }
        let Ok(handle) = world.workbench().cast_document::<TextBuffer>(document) else {
            return;
        };
        let pending = world
            .get_mut::<TextViewer>(panel)
            .and_then(|mut viewer| viewer.pending.take());
        let Ok(text) = world.workbench().read_document(handle) else {
            return;
        };
        let font = egui::FontId::monospace(context.metrics.font_size);
        let height = ui.fonts_mut(|fonts| fonts.row_height(&font));
        let mut scroll = egui::ScrollArea::both()
            .id_salt((panel, "text"))
            .auto_shrink([false, false]);
        if let Some(position) = pending {
            let line = match position {
                TextPosition::LineColumn { line, .. } => line as usize,
                TextPosition::Byte(byte) => text.line_of(byte.min(text.len())),
            };
            // A few lines of context above.
            scroll = scroll.vertical_scroll_offset(line.saturating_sub(3) as f32 * height);
        }
        let ink = bedtime_ui::theme::color(context.theme.palette.text);
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(4.0);
        scroll.show_rows(ui, height, text.line_count(), |ui, rows| {
            for row in rows {
                let line = text.range_text(text.line_range(row));
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(line.trim_end_matches(['\n', '\r']))
                                .font(font.clone())
                                .color(ink),
                        )
                        .extend(),
                    );
                });
            }
        });
    }
}
