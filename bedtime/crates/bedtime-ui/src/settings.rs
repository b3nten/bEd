//! Shared row presentation for provider-owned settings renderers.
use dear_imgui_rs::{TableFlags, TableOptions, TableSizingPolicy, Ui};

/// Align labels and controls, or stack them when the section is narrow.
/// The provider owns the value and decides which control to draw.
pub fn row(ui: &Ui, label: &str, draw: impl FnOnce() -> bool) -> bool {
    let _id = ui.push_id(label);
    let narrow = ui.content_region_avail()[0] < ui.current_font_size() * 25.0;
    let Some(_table) = ui.begin_table_with_flags(
        "##setting-row",
        if narrow { 1 } else { 2 },
        TableOptions::new()
            .flags(TableFlags::NO_SAVED_SETTINGS)
            .sizing_policy(TableSizingPolicy::StretchSame),
    ) else {
        return false;
    };
    ui.table_next_column();
    ui.align_text_to_frame_padding();
    ui.text_wrapped(label);
    ui.table_next_column();
    let _width = ui.push_item_width(-1.0);
    draw()
}

pub fn checkbox(ui: &Ui, label: &str, value: &mut bool) -> bool {
    row(ui, label, || ui.checkbox("##value", value))
}

pub fn number(ui: &Ui, label: &str, value: &mut f64) -> bool {
    row(ui, label, || ui.input_double("##value", value))
}

pub fn slider(ui: &Ui, label: &str, value: &mut f32, min: f32, max: f32) -> bool {
    // Imported preferences may exceed the normal UI range. Display them faithfully.
    row(ui, label, || {
        ui.slider_config("##value", min.min(*value), max.max(*value))
            .build(value)
    })
}
