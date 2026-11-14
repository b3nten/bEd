//! A hex viewer over bytes: offsets, hex and text columns (`HexView`), with
//! a find bar (bytes like "89 50 4E 47", or text), go to offset, a position
//! label and a menu copying the selection as hex, text or its offset
//! (`HexViewer`). Only the rows on screen are drawn.
use crate::{Floating, UiContext, egui, theme};

mod view;
pub use view::{
    BYTES_PER_ROW, HexColors, HexOutput, HexView, find, parse_hex, parse_offset, search_bytes,
};

impl HexColors {
    /// Byte colours from a theme.
    pub fn from_theme(theme: &theme::Theme) -> Self {
        let p = &theme.palette;
        Self {
            text: theme::color(p.text),
            muted: theme::color(p.muted),
            zero: theme::color(theme::mix(p.muted, p.panel_background, 0.45)),
            high: theme::color(theme::mix(p.accent, p.text, 0.35)),
            control: theme::color(theme::mix(p.warning, p.text, 0.3)),
            selection: theme::color(p.selection),
            match_fill: theme::color(theme::mix(p.warning, p.panel_background, 0.6)),
        }
    }
}

/// A hex view with its find and go-to bar. Keep one per bytes shown.
#[derive(Default)]
pub struct HexViewer {
    pub view: HexView,
    bar: Option<Bar>,
    rows: u64,
}

impl HexViewer {
    pub fn cursor(&self) -> u64 {
        self.view.cursor()
    }

    pub fn selection(&self, len: u64) -> std::ops::Range<u64> {
        self.view.selection(len)
    }

    /// Open the find bar.
    pub fn find(&mut self) {
        open_bar(self, Mode::Find);
    }

    /// Open the go-to-offset bar.
    pub fn go_to_offset(&mut self) {
        open_bar(self, Mode::GoTo);
    }

    /// Draw `bytes` filling the space left. With `keys` (give it while the
    /// caller has focus), Cmd/Ctrl+F finds and Cmd/Ctrl+G goes to an offset,
    /// and the view takes the keyboard.
    pub fn show(&mut self, ui: &mut egui::Ui, context: &UiContext<'_>, bytes: &[u8], keys: bool) {
        draw(ui, context, self, bytes, keys);
    }
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

fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    hex: &mut HexViewer,
    bytes: &[u8],
    focused: bool,
) {
    let len = bytes.len() as u64;
    let colors = HexColors::from_theme(context.theme);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let id = ui.id().with("bedtime.hex");
    let rect = ui.available_rect_before_wrap();
    let row_height = (font.size * 1.45).round();
    hex.rows = HexView::rows_in(rect.height(), row_height);
    // The last row scrolls clear of the position label.
    hex.view.end_padding = row_height * 1.5;
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

fn open_bar(hex: &mut HexViewer, mode: Mode) {
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
    frame: &mut crate::UiFrame<'_>,
    id: egui::Id,
    rect: egui::Rect,
    hex: &mut HexViewer,
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
                if f.floating_button(crate::BuiltinIcon::ChevronLeft, "Previous (Shift+Enter)")
                    .clicked()
                {
                    go = Some(false);
                }
                if f.floating_button(crate::BuiltinIcon::ChevronRight, "Next (Enter)")
                    .clicked()
                {
                    go = Some(true);
                }
            }
            if f.floating_button(crate::BuiltinIcon::Close, "Close (Escape)")
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

fn menu(ui: &mut egui::Ui, hex: &mut HexViewer, bytes: &[u8]) {
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
