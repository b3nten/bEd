use bed_editing::util::color::{blend, ensure_contrast};
use dear_imgui_rs::{StyleColor, TextureId, Ui};

/// Continue a control row only when the next item fits the available width.
pub fn same_line_if_fits(ui: &Ui, width: f32) {
    let right = ui.cursor_screen_pos()[0] + ui.content_region_avail()[0];
    if ui.item_rect_max()[0] + ui.clone_style().item_spacing()[0] + width <= right {
        ui.same_line();
    }
}

/// Fit a single-line label without splitting a Unicode character.
pub fn fit_text(ui: &Ui, text: &str, width: f32) -> String {
    let line = text.replace(['\r', '\n'], " ");
    if ui.calc_text_size(&line)[0] <= width {
        return line;
    }
    let ellipsis = ui.calc_text_size("…")[0];
    if width < ellipsis {
        return String::new();
    }
    let mut end = 0;
    for (offset, character) in line.char_indices() {
        let next = offset + character.len_utf8();
        if ui.calc_text_size(&line[..next])[0] > width - ellipsis {
            break;
        }
        end = next;
    }
    format!("{}…", &line[..end])
}

/// Readable secondary text for both light and dark host palettes.
pub fn muted_text_color(ui: &Ui) -> [f32; 4] {
    let background = ui.style_color(StyleColor::WindowBg);
    let text = ui.style_color(StyleColor::Text);
    ensure_contrast(blend(text, background, 0.6), background, 4.5)
}

/// Keep semantic hues while making their labels legible on the current surface.
pub fn readable_color(ui: &Ui, color: [f32; 4]) -> [f32; 4] {
    ensure_contrast(color, ui.style_color(StyleColor::WindowBg), 4.5)
}

/// Show a transient tool's captured directory without crowding its controls.
pub fn directory_label(ui: &Ui, directory: &str) {
    let width = ui.content_region_avail()[0];
    let mut label = directory.to_owned();
    if ui.calc_text_size(&label)[0] > width {
        let mut start = 0;
        for (index, _) in directory.char_indices() {
            start = index;
            if ui.calc_text_size(format!("…{}", &directory[start..]))[0] <= width {
                break;
            }
        }
        label = format!("…{}", &directory[start..]);
    }
    ui.text_disabled(label);
    if ui.is_item_hovered() {
        ui.tooltip_text(directory);
    }
}
pub trait FileIcons {
    fn get(&self, name: &str) -> Option<TextureId>;
    fn get_for_file(&self, filename: &str) -> Option<TextureId>;
    fn file_icon_tint(&self, _filename: &str, text: [f32; 4]) -> [f32; 4] {
        text
    }
}
