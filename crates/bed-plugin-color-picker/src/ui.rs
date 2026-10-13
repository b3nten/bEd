//! Shared presentation; callers own when color and palette changes are committed.
use super::{ColorDrafts, ColorState, color, read_color};
use dear_imgui_rs::{
    ColorButtonFlags, ColorInputMode, ColorPickerFlags, ColorPickerMode, StyleVar,
    TableColumnFlags, TableColumnWidth, TableFlags, TableOptions, TableSizingPolicy, Ui,
    WindowFlags,
};
use serde_json::{Value, json};

pub(super) fn draw_controls(
    ui: &Ui,
    state: &mut ColorState,
    drafts: &mut ColorDrafts,
    swatches: &mut Vec<Value>,
) -> bool {
    let mut changed = false;
    let _controls = bed_ui::components::controls_style(ui, &bed_ui::UiFrameContext::for_ui(ui));
    let fs = ui.current_font_size();
    let gap = fs * 0.5;
    let _spacing = ui.push_style_var(StyleVar::ItemSpacing([gap, fs * 0.35]));
    let _rounding = ui.push_style_var(StyleVar::FrameRounding(fs * 0.25));
    // Own scrolling so every control stays reachable even in a shallow host panel.
    ui.child_window("##color-tool")
        .size([0.0, 0.0])
        .flags(WindowFlags::NO_BACKGROUND)
        .build(ui, || {
            let available = ui.content_region_avail();
            let width = available[0].min(fs * 78.0).max(1.0);
            let wide = width >= fs * 34.0;
            let palette_column = width >= fs * 60.0;
            let shallow = available[1] < fs * 24.0;
            let _cell_padding = ui.push_style_var(StyleVar::CellPadding([
                gap, fs * if shallow { 0.08 } else { 0.25 },
            ]));
            let picker_limit = if wide {
                (available[1] - fs * 7.0).clamp(fs * 6.0, fs * 22.0)
            } else {
                (available[1] * 0.38).clamp(fs * 6.0, fs * 18.0)
            };
            let origin = ui.cursor_pos();
            ui.set_cursor_pos([
                origin[0] + (ui.content_region_avail()[0] - width) * 0.5,
                origin[1] + fs * 0.4,
            ]);
            // Resize changes geometry, never the color's editing or persistence semantics.
            let Some(layout) = ui.begin_table_with_sizing(
                "##color-layout",
                if palette_column { 3 } else if wide { 2 } else { 1 },
                TableOptions::new()
                    .flags(TableFlags::NO_SAVED_SETTINGS)
                    .sizing_policy(TableSizingPolicy::StretchProp),
                [width, 0.0],
                0.0,
            ) else {
                return;
            };
            ui.table_setup_column(
                "Picker",
                TableColumnFlags::NONE,
                Some(if wide {
                    TableColumnWidth::Fixed((width * 0.34).min(picker_limit + fs * 2.0))
                } else {
                    TableColumnWidth::Stretch(1.0)
                }),
            );
            if wide {
                ui.table_setup_column(
                    "Values and swatches",
                    TableColumnFlags::NONE,
                    Some(TableColumnWidth::Stretch(1.0)),
                );
            }
            if palette_column {
                ui.table_setup_column(
                    "Palette",
                    TableColumnFlags::NONE,
                    Some(TableColumnWidth::Stretch(0.85)),
                );
            }
            ui.table_next_column();
            ui.separator_with_text("Color");
            ui.color_button_config("##preview", state.rgba)
                .flags(ColorButtonFlags::NO_TOOLTIP)
                .size([ui.content_region_avail()[0], fs * if shallow { 1.5 } else { 2.6 }])
                .build();
            ui.text(&color::outputs(state.rgba)[0]);
            if ui.content_region_avail()[0] >= fs * 12.0 {
                ui.same_line();
            }
            ui.text_disabled(format!("{:.0}% opacity", state.rgba[3] * 100.0));
            ui.dummy([0.0, fs * 0.25]);
            // ImGui's hue and alpha bars share the picker's width; leave room for both.
            let picker_width = ui.content_region_avail()[0].min(picker_limit + fs * 2.0).max(1.0);
            let picker_origin = ui.cursor_pos();
            ui.set_cursor_pos([
                picker_origin[0] + (ui.content_region_avail()[0] - picker_width) * 0.5,
                picker_origin[1],
            ]);
            ui.set_next_item_width(picker_width);
            if ui
                .color_picker4_config("##picker", &mut state.rgba)
                .flags(
                    ColorPickerFlags::NO_INPUTS
                        | ColorPickerFlags::NO_SIDE_PREVIEW
                        | ColorPickerFlags::NO_SMALL_PREVIEW
                        | ColorPickerFlags::NO_LABEL
                        | ColorPickerFlags::NO_OPTIONS
                        | ColorPickerFlags::ALPHA_BAR,
                )
                .picker_mode(ColorPickerMode::HueBar)
                .input_mode(ColorInputMode::Rgb)
                .build()
            {
                drafts.fields = color::outputs(state.rgba);
                drafts.error = None;
                drafts.copied = None;
            }
            if ui.is_item_deactivated_after_edit() {
                state.remember();
            }
            if !state.recent.is_empty() {
                ui.set_cursor_pos_x(picker_origin[0]);
                ui.separator_with_text("Recent");
                let size = if shallow { fs * 1.1 } else { fs * 1.5 };
                let columns = ((ui.content_region_avail()[0] + gap) / (size + gap))
                    .floor()
                    .max(1.0) as usize;
                for (index, color) in state.recent.clone().into_iter().enumerate() {
                    if index % columns != 0 {
                        ui.same_line();
                    }
                    if ui
                        .color_button_config(format!("##recent{index}"), color)
                        .size([size; 2])
                        .build()
                    {
                        drafts.choose(state, color);
                    }
                }
            }
            ui.table_next_column();
            ui.separator_with_text("Values");
            let stacked_labels = ui.content_region_avail()[0] < fs * 18.0;
            let _values = ui.begin_table_with_flags(
                "##color-values",
                if stacked_labels { 1 } else { 3 },
                TableOptions::new()
                    .flags(TableFlags::NO_SAVED_SETTINGS)
                    .sizing_policy(TableSizingPolicy::StretchProp),
            );
            if _values.is_some() {
                if !stacked_labels {
                    ui.table_setup_column(
                        "Format",
                        TableColumnFlags::NONE,
                        Some(TableColumnWidth::Fixed(ui.calc_text_size("Float")[0])),
                    );
                }
                ui.table_setup_column(
                    "Value",
                    TableColumnFlags::NONE,
                    Some(TableColumnWidth::Stretch(1.0)),
                );
                if !stacked_labels {
                    ui.table_setup_column(
                        "Copy",
                        TableColumnFlags::NONE,
                        Some(TableColumnWidth::Fixed(
                            ui.calc_text_size("Copied")[0] + fs * 0.8,
                        )),
                    );
                }
                for (index, label) in color::LABELS.iter().enumerate() {
                    let _id = ui.push_id(index as i32);
                    ui.table_next_row();
                    ui.table_set_column_index(0);
                    if !stacked_labels {
                        ui.align_text_to_frame_padding();
                    }
                    ui.text_disabled(label);
                    if index == 6 && ui.is_item_hovered() {
                        bed_ui::components::tooltip_text(ui, &bed_ui::UiFrameContext::for_ui(ui), "Normalized RGBA components, from 0 to 1");
                    }
                    let copy_width = ui.calc_text_size("Copied")[0] + fs * 0.8;
                    if stacked_labels {
                        ui.same_line_with_pos(
                            ui.cursor_pos()[0] + ui.content_region_avail()[0] - copy_width,
                        );
                    } else {
                        ui.table_set_column_index(2);
                    }
                    let copied = drafts.copied.is_some_and(|(field, time)| {
                        field == index && ui.time() - time < 1.5
                    });
                    let button_width = if stacked_labels {
                        copy_width
                    } else {
                        ui.content_region_avail()[0]
                    };
                    if ui.button_with_size(
                        if copied { "Copied###copy" } else { "Copy###copy" },
                        [button_width, 0.0],
                    ) {
                        let text = std::ffi::CString::new(color::outputs(state.rgba)[index].as_str())
                            .expect("formatted colors contain no NUL");
                        ui.with_bound_context(|| unsafe {
                            dear_imgui_rs::sys::igSetClipboardText(text.as_ptr());
                        });
                        state.remember();
                        drafts.copied = Some((index, ui.time()));
                    }
                    #[cfg(test)]
                    {
                        let min = ui.item_rect_min();
                        let max = ui.item_rect_max();
                        drafts.copy_points[index] = [
                            (min[0] + max[0]) * 0.5,
                            (min[1] + max[1]) * 0.5,
                        ];
                    }
                    if ui.is_item_hovered() {
                        bed_ui::components::tooltip_text(ui, &bed_ui::UiFrameContext::for_ui(ui), format!("Copy {}", color::outputs(state.rgba)[index]));
                    }
                    if !stacked_labels {
                        ui.table_set_column_index(1);
                    }
                    ui.set_next_item_width(-1.0);
                    let edited = ui
                        .input_text(format!("##{label}"), &mut drafts.fields[index])
                        .build();
                    let finished = ui.is_item_deactivated_after_edit();
                    if ui.is_item_hovered() {
                        bed_ui::components::tooltip_text(ui, &bed_ui::UiFrameContext::for_ui(ui), &drafts.fields[index]);
                    }
                    if edited {
                        match color::parse(&drafts.fields[index], index, state.rgba[3]) {
                            Ok(rgba) => {
                                state.rgba = rgba;
                                drafts.error = None;
                                drafts.copied = None;
                                let formatted = color::outputs(rgba);
                                for (i, value) in formatted.into_iter().enumerate() {
                                    if i != index {
                                        drafts.fields[i] = value;
                                    }
                                }
                            }
                            Err(error) => drafts.error = Some(error),
                        }
                    }
                    if finished && drafts.error.is_none() {
                        drafts.fields = color::outputs(state.rgba);
                        state.remember();
                    }
                }
            }
            drop(_values);
            if let Some(error) = drafts.error {
                ui.text_wrapped(error);
            }
            if palette_column {
                ui.table_next_column();
            } else {
                ui.dummy([0.0, fs * 0.6]);
            }
            ui.separator_with_text("Palette");
            let columns = (ui.content_region_avail()[0] / (fs * 12.0))
                .floor()
                .max(1.0) as usize;
            let count = swatches
                .iter()
                .filter(|swatch| read_color(&swatch["rgba"]).is_some())
                .count();
            if count == 0 {
                let _muted = ui.push_style_color(
                    dear_imgui_rs::StyleColor::Text,
                    ui.style_color(dear_imgui_rs::StyleColor::TextDisabled),
                );
                ui.text_wrapped("Save a color to start your palette.");
            } else {
                let row_height = ui.frame_height() + fs * 0.65;
                let max_rows = if shallow { 2 } else { 5 };
                let rows = count.div_ceil(columns).min(max_rows);
                ui.child_window("##saved-swatches")
                    .size([0.0, rows as f32 * row_height])
                    .flags(WindowFlags::NO_BACKGROUND)
                    .build(ui, || {
                        let Some(_swatch_grid) = ui.begin_table_with_flags(
                            "##swatch-grid",
                            columns,
                            TableOptions::new()
                                .flags(TableFlags::NO_SAVED_SETTINGS)
                                .sizing_policy(TableSizingPolicy::StretchSame),
                        ) else {
                            return;
                        };
                        for (index, swatch) in swatches.iter().enumerate() {
                            let Some(rgba) = read_color(&swatch["rgba"]) else {
                                continue;
                            };
                            let _id = ui.push_id(index as i32);
                            ui.table_next_column();
                            let name = swatch["name"].as_str().unwrap_or("Color");
                            let clicked = ui
                                .color_button_config("##swatch", rgba)
                                .size([ui.frame_height(); 2])
                                .build();
                            ui.same_line();
                            let selected = ui
                                .selectable_config(format!("{name}##name"))
                                .selected(state.selected == Some(index))
                                .size([ui.content_region_avail()[0].max(1.0), ui.frame_height()])
                                .build();
                            if ui.is_item_hovered() {
                                bed_ui::components::tooltip_text(ui, &bed_ui::UiFrameContext::for_ui(ui), format!("{name}\n{}", color::outputs(rgba)[0]));
                            }
                            if clicked || selected {
                                drafts.choose(state, rgba);
                                state.selected = Some(index);
                                state.name = name.into();
                            }
                        }
                    });
            }
            let save_width = ui.calc_text_size("Save")[0] + fs * 1.1;
            ui.set_next_item_width((ui.content_region_avail()[0] - save_width - gap).max(1.0));
            ui.input_text("##swatch-name", &mut state.name)
                .hint("Swatch name (optional)")
                .build();
            ui.same_line();
            if ui.button_with_size("Save", [save_width, 0.0]) {
                let name = if state.name.trim().is_empty() {
                    color::outputs(state.rgba)[0].clone()
                } else {
                    state.name.trim().into()
                };
                swatches.push(json!({"name":name,"rgba":state.rgba}));
                state.selected = Some(swatches.len() - 1);
                changed = true;
            }
            if ui.is_item_hovered() {
                bed_ui::components::tooltip_text(ui, &bed_ui::UiFrameContext::for_ui(ui), "Save as a new swatch");
            }
            if let Some(index) = state.selected.filter(|index| *index < swatches.len()) {
                let button_width = (ui.content_region_avail()[0] - gap) * 0.5;
                if ui.button_with_size("Update", [button_width, 0.0]) {
                    swatches[index] = json!({"name":if state.name.trim().is_empty() { "Color" } else { state.name.trim() },"rgba":state.rgba});
                    changed = true;
                }
                if ui.is_item_hovered() {
                    bed_ui::components::tooltip_text(ui, &bed_ui::UiFrameContext::for_ui(ui), "Update the selected swatch with this color and name");
                }
                ui.same_line();
                if ui.button_with_size("Delete", [button_width, 0.0]) {
                    swatches.remove(index);
                    state.selected = None;
                    changed = true;
                }
            }
            drop(layout);
    });
    changed
}
