//! A document-free color tool. One RGBA value owns all text representations.
mod color;
use bed_editing::identity::DocumentId;
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, MenuSlot, Module, ModulePanel, PanelPlacement,
    Registrar,
};
use dear_imgui_rs::{
    ColorButtonFlags, ColorInputMode, ColorPickerFlags, ColorPickerMode, StyleVar,
    TableColumnFlags, TableColumnWidth, TableFlags, TableOptions, TableSizingPolicy, Ui,
    WindowFlags,
};
use serde_json::{Value, json};
use std::any::Any;

pub const MODULE_ID: &str = "bed.color";
pub const PANEL_ID: &str = "bed.color.panel";
pub const OPEN_COMMAND: &str = "bed.color.open";
pub struct ColorModule;
impl Module for ColorModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel_options(PANEL_ID, "Color Picker", true, PanelPlacement::Center, None);
        registrar.command(OPEN_COMMAND, "Color Picker", Some("image"));
        registrar.menu(MenuSlot::Application, OPEN_COMMAND);
        registrar.toolbar(OPEN_COMMAND);
    }
    fn command(
        &mut self,
        command: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if command == OPEN_COMMAND {
            requests.push(HostRequest::OpenPanel {
                panel_type: PANEL_ID.into(),
                document: None,
                state: Value::Null,
            });
        }
    }
    fn create_panel(
        &mut self,
        panel_type: &str,
        document: Option<DocumentId>,
        state: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if panel_type != PANEL_ID || document.is_some() {
            return Err("Color Picker is a tool without a document".into());
        }
        let rgba = read_color(&state["rgba"]).unwrap_or([0.45, 0.65, 1.0, 1.0]);
        Ok(Box::new(ColorPanel {
            rgba,
            fields: color::outputs(rgba),
            error: None,
            recent: state["recent"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(read_color)
                .take(16)
                .collect(),
            name: String::new(),
            selected: None,
            #[cfg(test)]
            copy_points: [[0.0; 2]; 7],
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
fn read_color(value: &Value) -> Option<[f32; 4]> {
    let values = value.as_array()?;
    if values.len() != 4 {
        return None;
    }
    let mut result = [0.0; 4];
    for (dst, src) in result.iter_mut().zip(values) {
        *dst = src.as_f64()? as f32;
    }
    color::valid(result).then_some(result)
}
struct ColorPanel {
    rgba: [f32; 4],
    fields: [String; 7],
    error: Option<&'static str>,
    recent: Vec<[f32; 4]>,
    name: String,
    selected: Option<usize>,
    #[cfg(test)]
    copy_points: [[f32; 2]; 7],
}
impl ColorPanel {
    fn remember(&mut self) {
        self.recent.retain(|color| *color != self.rgba);
        self.recent.insert(0, self.rgba);
        self.recent.truncate(16);
    }
    fn choose(&mut self, rgba: [f32; 4]) {
        self.rgba = rgba;
        self.fields = color::outputs(rgba);
        self.error = None;
        self.remember();
    }
}
impl ModulePanel for ColorPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Color Picker".into()
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        let fs = ui.current_font_size();
        let gap = fs * 0.5;
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([gap, fs * 0.35]));
        let _cell_padding = ui.push_style_var(StyleVar::CellPadding([gap, fs * 0.15]));
        let width = ui.content_region_avail()[0].min(fs * 62.0).max(1.0);
        let origin = ui.cursor_pos();
        ui.set_cursor_pos([
            origin[0] + (ui.content_region_avail()[0] - width) * 0.5,
            origin[1] + fs * 0.4,
        ]);
        // Only the column count changes on resize; all editing follows the same path.
        let wide = width >= fs * 40.0;
        let Some(layout) = ui.begin_table_with_sizing(
            "##color-layout",
            if wide { 2 } else { 1 },
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
                TableColumnWidth::Fixed(fs * 16.0)
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
        ui.table_next_column();
        ui.separator_with_text("Selected color");
        ui.color_button_config("##preview", self.rgba)
            .flags(ColorButtonFlags::NO_TOOLTIP)
            .size([fs * 2.6; 2])
            .build();
        ui.same_line();
        ui.group(|| {
            ui.text(&color::outputs(self.rgba)[0]);
            ui.text_disabled(format!("{:.0}% opacity", self.rgba[3] * 100.0));
        });
        // Keep the visual picker compact and put every text representation in one place.
        let picker_width = ui.content_region_avail()[0].min(fs * 16.0);
        let picker_origin = ui.cursor_pos();
        ui.set_cursor_pos([
            picker_origin[0] + (ui.content_region_avail()[0] - picker_width) * 0.5,
            picker_origin[1],
        ]);
        ui.set_next_item_width(picker_width);
        if ui
            .color_picker4_config("##picker", &mut self.rgba)
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
            self.fields = color::outputs(self.rgba);
            self.error = None;
        }
        if ui.is_item_deactivated_after_edit() {
            self.remember();
        }
        ui.set_cursor_pos_x(picker_origin[0]);
        if !self.recent.is_empty() {
            ui.separator_with_text("Recent");
            let size = ui.frame_height();
            let columns = ((ui.content_region_avail()[0] + gap) / (size + gap))
                .floor()
                .max(1.0) as usize;
            for (index, color) in self.recent.clone().into_iter().enumerate() {
                if index % columns != 0 {
                    ui.same_line();
                }
                if ui
                    .color_button_config(format!("##recent{index}"), color)
                    .size([size; 2])
                    .build()
                {
                    self.choose(color);
                }
            }
        }
        ui.table_next_column();
        ui.separator_with_text("Color values");
        let _values = ui.begin_table_with_flags(
            "##color-values",
            3,
            TableOptions::new()
                .flags(TableFlags::NO_SAVED_SETTINGS)
                .sizing_policy(TableSizingPolicy::StretchProp),
        );
        if _values.is_some() {
            ui.table_setup_column(
                "Format",
                TableColumnFlags::NONE,
                Some(TableColumnWidth::Fixed(ui.calc_text_size("Float")[0])),
            );
            ui.table_setup_column(
                "Value",
                TableColumnFlags::NONE,
                Some(TableColumnWidth::Stretch(1.0)),
            );
            ui.table_setup_column(
                "Copy",
                TableColumnFlags::NONE,
                Some(TableColumnWidth::Fixed(
                    ui.calc_text_size("Copy")[0] + fs * 1.1,
                )),
            );
            for (index, label) in color::LABELS.iter().enumerate() {
                let _id = ui.push_id(index as i32);
                ui.table_next_column();
                ui.align_text_to_frame_padding();
                ui.text_disabled(label);
                if index == 6 && ui.is_item_hovered() {
                    ui.tooltip_text("Normalized RGBA components, from 0 to 1");
                }
                ui.table_next_column();
                ui.set_next_item_width(-1.0);
                let edited = ui
                    .input_text(format!("##{label}"), &mut self.fields[index])
                    .build();
                let finished = ui.is_item_deactivated_after_edit();
                if ui.is_item_hovered() {
                    ui.tooltip_text(&self.fields[index]);
                }
                if edited {
                    match color::parse(&self.fields[index], index, self.rgba[3]) {
                        Ok(rgba) => {
                            self.rgba = rgba;
                            self.error = None;
                            let formatted = color::outputs(rgba);
                            for (i, value) in formatted.into_iter().enumerate() {
                                if i != index {
                                    self.fields[i] = value;
                                }
                            }
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                if finished && self.error.is_none() {
                    self.fields = color::outputs(self.rgba);
                    self.remember();
                }
                ui.table_next_column();
                if ui.button_with_size("Copy", [ui.content_region_avail()[0], 0.0]) {
                    let text = std::ffi::CString::new(color::outputs(self.rgba)[index].as_str())
                        .expect("formatted colors contain no NUL");
                    ui.with_bound_context(|| unsafe {
                        dear_imgui_rs::sys::igSetClipboardText(text.as_ptr());
                    });
                    self.remember();
                }
                #[cfg(test)]
                {
                    let min = ui.item_rect_min();
                    let max = ui.item_rect_max();
                    self.copy_points[index] = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text(format!("Copy {}", color::outputs(self.rgba)[index]));
                }
            }
        }
        drop(_values);
        if let Some(error) = self.error {
            ui.text_wrapped(error);
        }
        ui.dummy([0.0, fs * 0.3]);
        ui.separator_with_text("Saved swatches");
        let mut swatches = host.settings_for(MODULE_ID)["swatches"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut changed = false;
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
            let rows = count.div_ceil(columns).min(3);
            ui.child_window("##saved-swatches")
                .size([0.0, rows as f32 * (ui.frame_height() + fs * 0.65)])
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
                            .selected(self.selected == Some(index))
                            .size([ui.content_region_avail()[0].max(1.0), ui.frame_height()])
                            .build();
                        if ui.is_item_hovered() {
                            ui.tooltip_text(format!("{name}\n{}", color::outputs(rgba)[0]));
                        }
                        if clicked || selected {
                            self.choose(rgba);
                            self.selected = Some(index);
                            self.name = name.into();
                        }
                    }
                });
        }
        let save_width = ui.calc_text_size("Save")[0] + fs * 1.1;
        ui.set_next_item_width((ui.content_region_avail()[0] - save_width - gap).max(1.0));
        ui.input_text("##swatch-name", &mut self.name)
            .hint("Swatch name (optional)")
            .build();
        ui.same_line();
        if ui.button_with_size("Save", [save_width, 0.0]) {
            let name = if self.name.trim().is_empty() {
                color::outputs(self.rgba)[0].clone()
            } else {
                self.name.trim().into()
            };
            swatches.push(json!({"name":name,"rgba":self.rgba}));
            self.selected = Some(swatches.len() - 1);
            changed = true;
        }
        if ui.is_item_hovered() {
            ui.tooltip_text("Save as a new swatch");
        }
        if let Some(index) = self.selected.filter(|index| *index < swatches.len()) {
            let button_width = (ui.content_region_avail()[0] - gap) * 0.5;
            if ui.button_with_size("Update", [button_width, 0.0]) {
                swatches[index] = json!({"name":if self.name.trim().is_empty() { "Color" } else { self.name.trim() },"rgba":self.rgba});
                changed = true;
            }
            if ui.is_item_hovered() {
                ui.tooltip_text("Update the selected swatch with this color and name");
            }
            ui.same_line();
            if ui.button_with_size("Delete", [button_width, 0.0]) {
                swatches.remove(index);
                self.selected = None;
                changed = true;
            }
        }
        if changed {
            requests.push(HostRequest::SetSetting {
                plugin: MODULE_ID.into(),
                key: "swatches".into(),
                value: Value::Array(swatches),
            });
        }
        drop(layout);
    }
    fn save_state(&self) -> Value {
        json!({"rgba":self.rgba,"recent":self.recent})
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{
        ClipboardBackend, Condition, Context, FontSource, FramePrepareOptions, MouseButton,
    };
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    struct Clipboard(Rc<RefCell<String>>);
    impl ClipboardBackend for Clipboard {
        fn get(&mut self) -> Option<String> {
            Some(self.0.borrow().clone())
        }
        fn set(&mut self, value: &str) {
            *self.0.borrow_mut() = value.into();
        }
    }
    fn frame(
        context: &mut Context,
        panel: &mut dyn ModulePanel,
        settings: &Value,
        size: [f32; 2],
    ) -> usize {
        context.prepare_frame(FramePrepareOptions::new(size, 1.0 / 60.0));
        let ui = context.frame();
        let mut requests = Vec::new();
        ui.window("Color fixture")
            .position([0.0, 0.0], Condition::Always)
            .size(size, Condition::Always)
            .build(|| {
                panel.draw(
                    ui,
                    &HostContext {
                        remote: false,
                        default_viewers: &Value::Null,
                        viewer_menu: None,
                        documents: &[],
                        active_document: None,
                        settings,
                        textures: &HashMap::new(),
                        animations: false,
                        workspace: 1,
                        diagnostics: &Value::Null,
                    },
                    &mut requests,
                );
            });
        assert!(
            requests.is_empty(),
            "drawing/restoring must not rewrite saved swatches"
        );
        context.render_legacy().total_vtx_count()
    }
    #[test]
    fn panel_restores_without_changing_swatches_and_copies_every_output() {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(20.0)]);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let clipboard = Rc::new(RefCell::new(String::new()));
        context.set_clipboard_backend(Clipboard(clipboard.clone()));
        let rgba = [0.25, 0.5, 0.75, 0.5];
        let state = json!({"rgba":rgba,"recent":[[1.0,0.0,0.0,1.0]]});
        let settings =
            json!({MODULE_ID:{"swatches":[{"name":"Favorite","rgba":[0.0,1.0,0.0,1.0]}]}});
        let mut module = ColorModule;
        let mut panel = module.create_panel(PANEL_ID, None, &state).unwrap();
        assert!(frame(&mut context, panel.as_mut(), &settings, [1000.0; 2]) > 100);
        assert_eq!(panel.save_state(), state);
        assert!(panel.attached_document().is_none());
        // Every output remains reachable when a center panel becomes a narrow sidebar.
        for width in [1000.0, 400.0, 260.0] {
            let size = [width, 1000.0];
            frame(&mut context, panel.as_mut(), &settings, size);
            let expected = color::outputs(rgba);
            for (index, expected) in expected.into_iter().enumerate() {
                let point = panel
                    .as_any()
                    .downcast_ref::<ColorPanel>()
                    .unwrap()
                    .copy_points[index];
                assert!(
                    point[0] > 0.0 && point[0] < width,
                    "copy button must fit the panel"
                );
                assert!(
                    point[1] > 0.0 && point[1] < size[1],
                    "copy button must be visible"
                );
                context.io_mut().add_mouse_pos_event(point);
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, true);
                frame(&mut context, panel.as_mut(), &settings, size);
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, false);
                frame(&mut context, panel.as_mut(), &settings, size);
                assert_eq!(*clipboard.borrow(), expected);
            }
        }
        let saved = panel.save_state();
        let restored = module.create_panel(PANEL_ID, None, &saved).unwrap();
        assert_eq!(restored.save_state(), saved);
        assert_eq!(settings[MODULE_ID]["swatches"][0]["name"], "Favorite");
    }
}
