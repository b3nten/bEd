//! File-finder presentation around the GUI-free discovery service.
use bed_editing::util::color::blend;
use bed_files::file_finder::FileFinder as Finder;
use bed_ui::presentation::FileIcons;
use dear_imgui_rs::{Condition, Key, MouseButton, StyleColor, StyleVar, Ui, WindowFlags};
use std::{
    ops::{Deref, DerefMut},
    path::Path,
};

pub struct FileFinder {
    finder: Finder,
}
impl Default for FileFinder {
    fn default() -> Self {
        Self {
            finder: Finder::new(),
        }
    }
}
impl Deref for FileFinder {
    type Target = Finder;
    fn deref(&self) -> &Finder {
        &self.finder
    }
}
impl DerefMut for FileFinder {
    fn deref_mut(&mut self) -> &mut Finder {
        &mut self.finder
    }
}
impl FileFinder {
    pub fn new() -> Self {
        Self::default()
    }
}
#[derive(Clone, Copy, Debug)]
pub struct FileFinderStyle {
    pub background_color: [f32; 4],
    pub embedded_pane: Option<([f32; 2], [f32; 2])>,
}
impl Default for FileFinderStyle {
    fn default() -> Self {
        Self {
            background_color: [0.0, 0.0, 0.0, 1.0],
            embedded_pane: None,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FileFinderAction {
    #[default]
    None,
    Close,
    Open(String),
}
impl FileFinder {
    pub fn render_window(
        &mut self,
        ui: &Ui,
        style: &FileFinderStyle,
        icons: Option<&dyn FileIcons>,
    ) -> FileFinderAction {
        if self.show_ff_window && ui.is_key_pressed(Key::Escape) {
            self.cancel_and_close();
            return FileFinderAction::Close;
        }
        if !self.show_ff_window {
            return FileFinderAction::None;
        }
        let fs = ui.current_font_size();
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        let size = [fs * 30.0, fs * 17.5];
        let (position, pivot) = if let Some((pos, pane_size)) = style
            .embedded_pane
            .filter(|(_, size)| size[0] > 0.0 || size[1] > 0.0)
        {
            (
                [
                    pos[0] + pane_size[0] * 0.5 - size[0] * 0.5,
                    pos[1] + pane_size[1] * 0.5 - size[1] * 0.5,
                ],
                [0.0; 2],
            )
        } else {
            let display = ui.io().display_size();
            ([display[0] * 0.5, display[1] * 0.35], [0.5; 2])
        };
        let text = ui.style_color(StyleColor::Text);
        let background = style.background_color;
        let dimmed = blend(
            text,
            [background[0], background[1], background[2], 1.0],
            0.025,
        );
        let _rounding = ui.push_style_var(StyleVar::WindowRounding(fs * 0.5));
        let _border_size = ui.push_style_var(StyleVar::WindowBorderSize(1.0));
        let _padding = ui.push_style_var(StyleVar::WindowPadding([fs * 0.8; 2]));
        let _bg = ui.push_style_color(StyleColor::WindowBg, dimmed);
        let _border = ui.push_style_color(StyleColor::Border, blend(text, dimmed, 0.30));
        let _frame_bg = ui.push_style_color(StyleColor::FrameBg, dimmed);
        let mut action = FileFinderAction::None;
        ui.window("FileFinder")
            .size(size, Condition::Always)
            .position(
                [
                    position[0] - size[0] * pivot[0],
                    position[1] - size[1] * pivot[1],
                ],
                Condition::Always,
            )
            .flags(
                WindowFlags::NO_TITLE_BAR
                    | WindowFlags::NO_RESIZE
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_SCROLLBAR
                    | WindowFlags::NO_SCROLL_WITH_MOUSE
                    | WindowFlags::NO_DOCKING,
            )
            .build(|| {
                ui.text("Find File");
                ui.spacing();
                ui.spacing();
                if ui.is_key_pressed(Key::UpArrow) && self.selected_index > 0 {
                    self.selected_index -= 1;
                }
                if ui.is_key_pressed(Key::DownArrow)
                    && self.selected_index + 1 < self.filtered_list.len()
                {
                    self.selected_index += 1;
                }
                if ui.is_mouse_clicked(MouseButton::Left) {
                    let pos = ui.window_pos();
                    let size = ui.window_size();
                    let mouse = ui.io().mouse_pos();
                    if mouse[0] < pos[0]
                        || mouse[0] > pos[0] + size[0]
                        || mouse[1] < pos[1]
                        || mouse[1] > pos[1] + size[1]
                    {
                        self.cancel_and_close();
                        action = FileFinderAction::Close;
                        return;
                    }
                }
                let enter = {
                    let _round = ui.push_style_var(StyleVar::FrameRounding(fs * 0.2));
                    let _frame_border = ui.push_style_var(StyleVar::FrameBorderSize(1.0));
                    let _frame_pad = ui.push_style_var(StyleVar::FramePadding([fs * 0.4; 2]));
                    let _border =
                        ui.push_style_color(StyleColor::Border, blend(text, dimmed, 0.30));
                    let _frame = ui.push_style_color(StyleColor::FrameBg, dimmed);
                    ui.set_next_item_width(ui.content_region_avail()[0]);
                    ui.set_keyboard_focus_here();
                    ui.input_text("##SearchInput", &mut self.search_buffer)
                        .auto_select_all(true)
                        .enter_returns_true(true)
                        .build()
                };
                if enter {
                    action = self
                        .commit_selection()
                        .map(FileFinderAction::Open)
                        .unwrap_or(FileFinderAction::Close);
                    return;
                }
                let query = self.search_buffer.clone();
                self.set_query(&query);
                ui.spacing();
                ui.spacing();
                ui.dummy([0.0, fs * 0.5]);
                self.render_file_list(ui, icons);
                ui.separator();
                ui.text("Press Ctrl+P or ESC to close");
            });
        action
    }
    fn render_file_list(&self, ui: &Ui, icons: Option<&dyn FileIcons>) {
        let height = ui.text_line_height_with_spacing();
        let visible = (ui.content_region_avail()[1] / height).max(1.0) as usize;
        let total = self.filtered_list.len();
        let mut start = self.selected_index.saturating_sub(visible / 2);
        let end = total.min(start.saturating_add(visible));
        if end == total {
            start = total.saturating_sub(visible);
        }
        let _bg = ui.push_style_color(StyleColor::ChildBg, [0.0; 4]);
        let _border = ui.push_style_color(StyleColor::Border, [0.0; 4]);
        ui.child_window("SearchResults")
            .size([0.0, -ui.frame_height_with_spacing()])
            .flags(
                WindowFlags::NO_SCROLLBAR
                    | WindowFlags::NO_SCROLL_WITH_MOUSE
                    | WindowFlags::NO_MOUSE_INPUTS,
            )
            .build(ui, || {
                let fs = ui.current_font_size();
                let _align = ui.push_style_var(StyleVar::SelectableTextAlign([0.0, 0.5]));
                let _round = ui.push_style_var(StyleVar::FrameRounding(fs * 0.2));
                let _pad = ui.push_style_var(StyleVar::FramePadding([fs * 0.4, fs * 0.2]));
                let _hover = ui.push_style_color(StyleColor::HeaderHovered, [0.0; 4]);
                let _active = ui.push_style_color(StyleColor::HeaderActive, [0.0; 4]);
                for i in start..end {
                    let entry = &self.filtered_list[i];
                    let selected = i == self.selected_index;
                    let _id = ui.push_id(i as i32);
                    ui.selectable_config("")
                        .selected(selected)
                        .span_all_columns(true)
                        .build();
                    ui.same_line();
                    let filename = Path::new(&entry.full_path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default();
                    let icon_size = ui.text_line_height();
                    if let Some(icon) = icons.and_then(|icons| icons.get_for_file(filename)) {
                        let tint = if icons.is_some_and(|icons| Some(icon) == icons.get("default"))
                        {
                            ui.style_color(StyleColor::Text)
                        } else {
                            [1.0; 4]
                        };
                        ui.image_config(icon, [icon_size; 2])
                            .tint_color(tint)
                            .build();
                    } else {
                        ui.dummy([icon_size; 2]);
                    }
                    ui.same_line();
                    ui.text(&entry.relative_path);
                    if selected {
                        ui.set_scroll_here_y(0.5);
                    }
                }
            });
    }
}
