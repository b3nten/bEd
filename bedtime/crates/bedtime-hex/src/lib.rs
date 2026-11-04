//! A hex viewer for workbench apps: any file's bytes (up to 256 MiB) as
//! offsets, hex and text. Cmd/Ctrl+F finds bytes ("89 50 4E 47") or text,
//! Cmd/Ctrl+G goes to an offset; the right-click menu copies the selection
//! as hex, text or its offset. It opens any file, so it's the viewer of last
//! resort for files nothing else shows.
use bedtime_document::DocumentModel;
use bedtime_filesystem::FileModel;
use bedtime_ui::{Floating, UiContext, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    DocumentHandle, Fit, Opener, OpenersAppExt, Panel, PanelPresentation, Viewer, viewer_document,
};
use bevy::prelude::*;
use std::sync::Arc;

mod view;
pub use view::{BYTES_PER_ROW, HexColors, HexView, find, parse_hex, parse_offset, search_bytes};

pub struct HexPlugin;

impl Plugin for HexPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<HexPanel>()
            .register_opener(Opener::<Bytes>::new::<HexPanel>("Hex").files(|_, _| Fit::Can));
    }
}

/// A file's bytes, read-only.
#[derive(Clone, Default, PartialEq)]
pub struct Bytes(pub Arc<[u8]>);

impl DocumentModel for Bytes {
    type Change = std::convert::Infallible;
    type OpenState = ();
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String> {
        match *change {}
    }
}

impl FileModel for Bytes {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        Ok(Self(bytes.into()))
    }
    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        out.write_all(&self.0)
    }
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct HexPanel {
    #[reflect(ignore)]
    view: HexView,
    #[reflect(ignore)]
    bar: Option<Bar>,
    #[reflect(ignore)]
    rows: u64,
}

/// The box at the top right: finding, or going to an offset.
struct Bar {
    mode: Mode,
    text: String,
    focus: bool,
    message: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Find,
    GoTo,
}

impl HexPanel {
    pub fn cursor(&self) -> u64 {
        self.view.cursor()
    }
    pub fn selection(&self, len: u64) -> std::ops::Range<u64> {
        self.view.selection(len)
    }
}

impl Viewer<Bytes> for HexPanel {
    fn new(_: DocumentHandle<Bytes>) -> Self {
        Self::default()
    }
    fn apply(_: &mut World, _: Entity, _: &()) {}
}

impl Panel for HexPanel {
    const LABEL: &'static str = "Hex";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<Bytes>(ui, world, panel) else {
            return;
        };
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|presentation| presentation.focused);
        let Some(mut hex) = world.get_mut::<HexPanel>(panel) else {
            return;
        };
        draw(ui, context, panel, &mut hex, &document.model.0, focused);
    }
}

fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    hex: &mut HexPanel,
    bytes: &[u8],
    focused: bool,
) {
    let len = bytes.len() as u64;
    let p = &context.theme.palette;
    let colors = HexColors {
        text: theme::color(p.text),
        muted: theme::color(p.muted),
        zero: theme::color(theme::mix(p.muted, p.panel_background, 0.45)),
        high: theme::color(theme::mix(p.accent, p.text, 0.35)),
        control: theme::color(theme::mix(p.warning, p.text, 0.3)),
        selection: theme::color(p.selection),
        match_fill: theme::color(theme::mix(p.warning, p.panel_background, 0.6)),
    };
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let id = egui::Id::new(("bedtime.hex", panel));
    let rect = ui.available_rect_before_wrap();
    let row_height = (font.size * 1.45).round();
    hex.rows = HexView::rows_in(rect.height(), row_height);
    // Cmd/Ctrl+F and G open the bar while the panel has focus.
    if focused && !egui::Popup::is_any_open(ui.ctx()) {
        let (find, go) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::F),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::G),
            )
        });
        if find {
            open_bar(hex, Mode::Find);
        }
        if go {
            open_bar(hex, Mode::GoTo);
        }
    }
    let output = hex.view.show(ui, id, bytes, font, colors);
    if focused && hex.bar.is_none() && !ui.ctx().egui_wants_keyboard_input() {
        output.response.request_focus();
    }
    if output.copy {
        ui.ctx().copy_text(copy_hex(bytes, hex.view.selection(len)));
    }
    let mut frame = context.frame(ui);
    output.response.context_menu(|ui| menu(ui, hex, bytes));
    // Where the cursor is, and how much is selected.
    let selection = hex.view.selection(len);
    let position = match selection.end - selection.start {
        0 | 1 => format!("0x{:X}", hex.view.cursor()),
        n => format!("0x{:X} · {n} bytes", selection.start),
    };
    Floating::new(id.with("position"), rect, egui::Align2::RIGHT_BOTTOM).show(&mut frame, |f| {
        f.floating_label(position);
    });
    if hex.bar.is_some() {
        draw_bar(&mut frame, id, rect, hex, bytes);
    }
}

fn open_bar(hex: &mut HexPanel, mode: Mode) {
    let text = match &hex.bar {
        Some(bar) if bar.mode == mode => bar.text.clone(),
        _ => String::new(),
    };
    hex.bar = Some(Bar {
        mode,
        text,
        focus: true,
        message: None,
    });
}

fn draw_bar(
    frame: &mut bedtime_ui::UiFrame<'_>,
    id: egui::Id,
    rect: egui::Rect,
    hex: &mut HexPanel,
    bytes: &[u8],
) {
    let len = bytes.len() as u64;
    let rows = hex.rows;
    let Some(bar) = hex.bar.as_mut() else {
        return;
    };
    let mut close = false;
    let mut go: Option<bool> = None;
    Floating::new(id.with("bar"), rect, egui::Align2::RIGHT_TOP)
        .shown(true)
        .show(frame, |f| {
            let hint = match bar.mode {
                Mode::Find => "Find bytes or text",
                Mode::GoTo => "Go to offset",
            };
            let edit = egui::TextEdit::singleline(&mut bar.text)
                .id(id.with("bar-text"))
                .hint_text(hint)
                .desired_width(180.0)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::symmetric(6, 3));
            let response = f.ui.add(edit);
            if std::mem::take(&mut bar.focus) {
                response.request_focus();
            }
            let (enter, shift, escape) = f.ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Enter),
                    i.modifiers.shift,
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if response.lost_focus() && enter {
                go = Some(!shift);
                response.request_focus();
            }
            if escape && (response.has_focus() || response.lost_focus()) {
                close = true;
            }
            if let Some(message) = &bar.message {
                f.floating_label(message.clone());
            }
            if bar.mode == Mode::Find {
                if f.floating_button(
                    bedtime_ui::BuiltinIcon::ChevronLeft,
                    "Previous (Shift+Enter)",
                )
                .clicked()
                {
                    go = Some(false);
                }
                if f.floating_button(bedtime_ui::BuiltinIcon::ChevronRight, "Next (Enter)")
                    .clicked()
                {
                    go = Some(true);
                }
            }
            if f.floating_button(bedtime_ui::BuiltinIcon::Close, "Close (Escape)")
                .clicked()
            {
                close = true;
            }
        });
    if let Some(forward) = go {
        match bar.mode {
            Mode::Find => {
                let needle = search_bytes(&bar.text);
                let selection = hex.view.selection(len);
                let from = if forward {
                    selection.start + 1
                } else {
                    selection.start
                };
                match find(bytes, &needle, from.min(len), forward) {
                    Some(at) => {
                        let found = at..at + needle.len() as u64;
                        hex.view.select(found.clone(), len, rows);
                        hex.view.found = Some(found);
                        bar.message = None;
                    }
                    None => {
                        hex.view.found = None;
                        bar.message = Some("Not found".into());
                    }
                }
            }
            Mode::GoTo => match parse_offset(&bar.text) {
                Ok(offset) if offset < len.max(1) => {
                    hex.view.go_to(offset, len, rows);
                    close = true;
                }
                Ok(_) => bar.message = Some("Past the end".into()),
                Err(_) => bar.message = Some("Not an offset".into()),
            },
        }
    }
    if close {
        hex.bar = None;
        hex.view.found = None;
    }
}

fn menu(ui: &mut egui::Ui, hex: &mut HexPanel, bytes: &[u8]) {
    let len = bytes.len() as u64;
    let selection = hex.view.selection(len);
    if ui.button("Copy as Hex").clicked() {
        ui.ctx().copy_text(copy_hex(bytes, selection.clone()));
    }
    if ui.button("Copy as Text").clicked() {
        let range = selection.start as usize..selection.end as usize;
        ui.ctx()
            .copy_text(String::from_utf8_lossy(&bytes[range]).into_owned());
    }
    if ui.button("Copy Offset").clicked() {
        ui.ctx().copy_text(format!("0x{:X}", selection.start));
    }
    ui.separator();
    if ui.button("Select All").clicked() {
        hex.view.select_all(len);
    }
    if ui.button("Find…").clicked() {
        open_bar(hex, Mode::Find);
    }
    if ui.button("Go to Offset…").clicked() {
        open_bar(hex, Mode::GoTo);
    }
}

fn copy_hex(bytes: &[u8], range: std::ops::Range<u64>) -> String {
    bytes[range.start as usize..range.end as usize]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
